//! Formatting for the live VFS mount view. No mount records are invented here.

use std::fmt::Write;

use crate::fs::object::Object;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
    GetVolumeInformationByHandleW,
};

// FILE_READ_ONLY_VOLUME from winnt.h; windows-sys exposes this bit in a
// SystemServices feature that is otherwise unnecessary for the VFS.
const HOST_READ_ONLY_VOLUME: u32 = 0x0008_0000;

/// One resolved attachment. The producer supplies real identities and policy;
/// formatters never infer Linux filesystem capabilities from pathnames.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Record {
    pub idmap: Option<crate::user_namespace::Mapping>,
    pub peer: u64,
    pub master: u64,
    pub id: u64,
    pub parent: u64,
    pub device: u64,
    pub root: String,
    pub target: String,
    pub filesystem: String,
    pub source: String,
    pub options: String,
    pub super_options: String,
    pub flags: u64,
}

/// The bridge exposes native volumes through its own VFS, not an ext4 driver.
/// Query a pinned object so the device agrees with fs::Stat::st_dev and the
/// displayed root names the actual directory inside that native volume.
pub(crate) fn host_record(
    path: &std::path::Path,
    id: u64,
    parent: u64,
    target: String,
    flags: u64,
) -> Result<Record, i32> {
    use std::path::{Component, Prefix};

    let object = Object::open(path, FILE_READ_ATTRIBUTES)?;
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(object.raw(), &mut information) } == 0 {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let mut volume_flags = 0;
    if unsafe {
        GetVolumeInformationByHandleW(
            object.raw(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut volume_flags,
            std::ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let actual = object.path()?;
    let mut source = None;
    let mut root = String::new();
    for component in actual.components() {
        match component {
            Component::Prefix(prefix) => {
                source = Some(match prefix.kind() {
                    Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                        format!("{}:", char::from(letter).to_ascii_uppercase())
                    }
                    Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                        "//{}/{}",
                        server.to_str().ok_or(crate::EIO)?,
                        share.to_str().ok_or(crate::EIO)?
                    ),
                    _ => prefix.as_os_str().to_str().ok_or(crate::EIO)?.to_owned(),
                });
            }
            Component::RootDir => {}
            Component::Normal(name) => {
                root.push('/');
                root.push_str(&crate::path::unescape_path(
                    name.to_str().ok_or(crate::EIO)?,
                ));
            }
            Component::CurDir | Component::ParentDir => return Err(crate::EIO),
        }
    }
    if root.is_empty() {
        root.push('/');
    }
    let options = if volume_flags & HOST_READ_ONLY_VOLUME != 0 || flags & 1 != 0 {
        "ro"
    } else {
        "rw"
    };
    Ok(Record {
        idmap: None,
        peer: 0,
        master: 0,
        id,
        parent,
        device: u64::from(information.dwVolumeSerialNumber),
        root,
        target,
        filesystem: "kinakaze".into(),
        source: source.ok_or(crate::EIO)?,
        options: options.into(),
        super_options: options.into(),
        flags,
    })
}

pub(crate) fn records(pid: u32) -> Result<Vec<Record>, i32> {
    if pid != crate::job::process_id() {
        // The shared PID record does not publish this process's guest root.
        // Another loader may use a completely different root in the same
        // Windows session. Substituting the reader's root would be false data.
        return Err(crate::EOPNOTSUPP);
    }
    let root = crate::system_root().map_err(|_| crate::EIO)?;
    let overlay_root = crate::path::overlay_root();
    let namespace_root = crate::path::namespace_root_path()?;
    let base = crate::path::default_system_root();
    let native_base = overlay_root
        .as_ref()
        .map(|r| r.native_base.as_path())
        .unwrap_or_else(|| {
            if namespace_root.is_some() {
                &base
            } else {
                &root
            }
        });
    let points = crate::mount::report_snapshot()?;
    let mut records = Vec::with_capacity(points.len() + 1);
    records.push(host_record(
        native_base,
        crate::mount::ROOT_MOUNT_ID,
        crate::mount::ROOT_MOUNT_ID,
        "/".into(),
        0,
    )?);
    for point in &points {
        if point.id == crate::mount::ROOT_MOUNT_ID {
            records[0].flags = point.flags;
            records[0].peer = point.peer;
            records[0].master = point.master;
            continue;
        }
        if point.flags & crate::mount::MS_TMPFS != 0 {
            let (volume, node) = crate::tmpfs::parse(&point.source)?;
            let mut options = if point.flags & 1 != 0 { "ro" } else { "rw" }.to_owned();
            for (flag, name) in [
                (2, "nosuid"),
                (4, "nodev"),
                (8, "noexec"),
                (1024, "noatime"),
                (2048, "nodiratime"),
                (1 << 21, "relatime"),
                (1 << 24, "strictatime"),
            ] {
                if point.flags & flag != 0 {
                    options.push(',');
                    options.push_str(name);
                }
            }
            records.push(Record {
                idmap: None,
                peer: point.peer,
                master: point.master,
                id: point.id,
                parent: point.parent,
                device: crate::tmpfs::device(volume),
                root: if point.flags & crate::mount::MS_CGROUP != 0 {
                    crate::tmpfs::cgroupfs::mount_root(&point.source)?
                } else if node == 1 {
                    "/".into()
                } else {
                    format!("/inode/{node}")
                },
                target: point.target.clone(),
                filesystem: crate::tmpfs::filesystem(volume)?.into(),
                source: crate::tmpfs::filesystem(volume)?.into(),
                options,
                super_options: crate::tmpfs::options(volume)?,
                flags: point.flags,
            });
            continue;
        }
        if point.flags & crate::mount::MS_PROC != 0 {
            let (view, tail) = super::instance::parse(&point.source)?;
            let mut options = if point.flags & 1 != 0 { "ro" } else { "rw" }.to_owned();
            for (flag, name) in [
                (2, "nosuid"),
                (4, "nodev"),
                (8, "noexec"),
                (1024, "noatime"),
                (2048, "nodiratime"),
                (1 << 21, "relatime"),
                (1 << 24, "strictatime"),
            ] {
                if point.flags & flag != 0 {
                    options.push(',');
                    options.push_str(name);
                }
            }
            records.push(Record {
                idmap: None,
                peer: point.peer,
                master: point.master,
                id: point.id,
                parent: point.parent,
                device: view.device(),
                root: format!("/{tail}"),
                target: point.target.clone(),
                filesystem: "proc".into(),
                source: "proc".into(),
                super_options: if view.subset_pid {
                    "rw,subset=pid"
                } else {
                    "rw"
                }
                .into(),
                options,
                flags: point.flags,
            });
            continue;
        }
        if point.flags & crate::mount::MS_OVERLAY != 0 {
            let (device, readonly) = crate::mount::overlay::mount_metadata(&point.source)?;
            let mut options = if readonly || point.flags & 1 != 0 {
                "ro"
            } else {
                "rw"
            }
            .to_owned();
            options.push_str(if point.flags & 1024 != 0 {
                ",noatime"
            } else if point.flags & (1 << 24) != 0 {
                ",strictatime"
            } else {
                ",relatime"
            });
            if point.flags & 2048 != 0 {
                options.push_str(",nodiratime");
            }
            let parsed = crate::mount::ParsedOverlay::parse(&point.source).ok_or(crate::EIO)?;
            let split = parsed
                .lowerdirs
                .len()
                .checked_sub(crate::mount::overlay::features::data_count(point.flags))
                .ok_or(crate::EIO)?;
            let mut lowerdirs = parsed.lowerdirs[..split]
                .iter()
                .map(|path| option_path(path))
                .collect::<Vec<_>>()
                .join(":");
            for data in &parsed.lowerdirs[split..] {
                lowerdirs.push_str("::");
                lowerdirs.push_str(&option_path(data));
            }
            let mut super_options = format!(
                "{},lowerdir={lowerdirs}",
                if readonly { "ro" } else { "rw" }
            );
            if let Some(upper) = parsed.upperdir {
                super_options.push_str(&format!(",upperdir={}", option_path(&upper)));
            }
            if let Some(work) = parsed.workdir {
                super_options.push_str(&format!(",workdir={}", option_path(&work)));
            }
            super_options.push(',');
            super_options.push_str(&crate::mount::overlay::feature_options(&point.source)?);
            let (_, view) = crate::mount::overlay::split_view(&point.source)?;
            records.push(Record {
                idmap: point.idmap.clone(),
                peer: point.peer,
                master: point.master,
                id: point.id,
                parent: point.parent,
                device,
                root: format!("/{view}"),
                target: point.target.clone(),
                filesystem: "overlay".into(),
                source: "overlay".into(),
                options: options.into(),
                super_options,
                flags: point.flags,
            });
            continue;
        }
        let source = crate::path::resolve_bound_mount_source(native_base, &point.source).map_err(
            |error| match error {
                crate::path::PathError::Filesystem(error) => error,
                _ => crate::EIO,
            },
        )?;
        records.push(host_record(
            &source,
            point.id,
            point.parent,
            point.target.clone(),
            point.flags,
        )?);
        if let Some(record) = records.last_mut() {
            record.peer = point.peer;
            record.master = point.master;
        }
    }
    if let Some(namespace_root) = &namespace_root {
        let mut visible_root = if let Some(id) = crate::mount::visible_mount_id(namespace_root)? {
            let point = points
                .iter()
                .find(|point| point.id == id)
                .ok_or(crate::EAGAIN)?;
            let mut visible = records
                .iter()
                .find(|record| record.id == point.id)
                .cloned()
                .ok_or(crate::EIO)?;
            let suffix = namespace_root
                .strip_prefix(&point.target)
                .ok_or(crate::EIO)?;
            visible.root = format!(
                "{}/{}",
                visible.root.trim_end_matches('/'),
                suffix.trim_start_matches('/')
            );
            if visible.root.len() > 1 {
                visible.root = visible.root.trim_end_matches('/').into();
            }
            visible
        } else {
            let inherited = &records[0];
            let mut visible = host_record(
                &root,
                inherited.id,
                inherited.parent,
                "/".into(),
                inherited.flags,
            )?;
            visible.peer = inherited.peer;
            visible.master = inherited.master;
            visible
        };
        visible_root.target = "/".into();
        let prefix = format!("{}/", namespace_root.trim_end_matches('/'));
        records.retain(|record| record.target.starts_with(&prefix) && record.id != visible_root.id);
        for record in &mut records {
            record.target = format!(
                "/{}",
                record.target.strip_prefix(&prefix).ok_or(crate::EIO)?
            );
        }
        records.insert(0, visible_root);
    }
    // Native pathname queries can block and must not hold the shared writer
    // mutex. Reject a topology/root change rather than combining generations.
    if points != crate::mount::report_snapshot()?
        || root != crate::system_root().map_err(|_| crate::EIO)?
        || overlay_root != crate::path::overlay_root()
        || namespace_root != crate::path::namespace_root_path()?
    {
        return Err(crate::EAGAIN);
    }
    Ok(records)
}

/// Linux's new_encode_dev layout, also used by this project's Stat::st_dev.
fn device_numbers(device: u64) -> (u64, u64) {
    (
        ((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000),
        (device & 0xff) | ((device >> 12) & 0xffff_ff00),
    )
}

fn option_path(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            ' ' => result.push_str("\\040"),
            '\t' => result.push_str("\\011"),
            '\n' => result.push_str("\\012"),
            '\\' => result.push_str("\\134"),
            ',' => result.push_str("\\054"),
            ':' => result.push_str("\\072"),
            _ => result.push(ch),
        }
    }
    result
}

fn escaped(value: &str, hash: bool) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            ' ' => result.push_str("\\040"),
            '\t' => result.push_str("\\011"),
            '\n' => result.push_str("\\012"),
            '\\' => result.push_str("\\134"),
            '#' if hash => result.push_str("\\043"),
            other => result.push(other),
        }
    }
    result
}

pub(super) fn mountinfo(records: &[Record]) -> String {
    let mut output = String::new();
    for record in records {
        let (major, minor) = device_numbers(record.device);
        let mut optional = String::new();
        if record.flags & crate::mount::MS_SHARED != 0 {
            optional.push_str(&format!("shared:{} ", record.peer));
        }
        if record.flags & crate::mount::MS_SLAVE != 0 {
            optional.push_str(&format!("master:{} ", record.master));
        } else if record.flags & crate::mount::MS_UNBINDABLE != 0 {
            optional.push_str("unbindable ");
        }
        let _ = writeln!(
            output,
            "{} {} {major}:{minor} {} {} {} {}- {} {} {}",
            record.id,
            record.parent,
            escaped(&record.root, false),
            escaped(&record.target, false),
            record.options,
            optional,
            escaped(&record.filesystem, true),
            escaped(&record.source, true),
            record.super_options,
        );
    }
    output
}

pub(super) fn mounts(records: &[Record]) -> String {
    let mut output = String::new();
    for record in records {
        let mut options = record.options.clone();
        for option in record.super_options.split(',') {
            if !options.split(',').any(|present| present == option) {
                options.push(',');
                options.push_str(option);
            }
        }
        let _ = writeln!(
            output,
            "{} {} {} {options} 0 0",
            escaped(&record.source, true),
            escaped(&record.target, false),
            escaped(&record.filesystem, true),
        );
    }
    output
}

pub(super) fn mountstats(records: &[Record]) -> String {
    let mut output = String::new();
    for record in records {
        // There is no filesystem-specific statistics backend. Linux's common
        // mountstats record is still valid without optional statistics.
        let _ = writeln!(
            output,
            "device {} mounted on {} with fstype {}",
            escaped(&record.source, true),
            escaped(&record.target, false),
            escaped(&record.filesystem, true),
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            idmap: None,
            peer: 0,
            master: 0,
            id: 17,
            parent: 3,
            device: 0x801,
            root: "/lower tree/中文".into(),
            target: "/mounted\tname\nslash\\end".into(),
            filesystem: "kinakaze".into(),
            source: "volume#one\\".into(),
            options: "rw".into(),
            super_options: "rw".into(),
            flags: 0,
        }
    }

    #[test]
    fn mountinfo_escapes_fields_without_injecting_records_or_capabilities() {
        assert_eq!(
            mountinfo(&[record()]),
            "17 3 8:1 /lower\\040tree/中文 /mounted\\011name\\012slash\\134end rw - kinakaze volume\\043one\\134 rw\n"
        );
    }

    #[test]
    fn mounts_and_mountstats_use_the_same_records_and_linux_formats() {
        let records = [record()];
        assert_eq!(
            mounts(&records),
            "volume\\043one\\134 /mounted\\011name\\012slash\\134end kinakaze rw 0 0\n"
        );
        assert_eq!(
            mountstats(&records),
            "device volume\\043one\\134 mounted on /mounted\\011name\\012slash\\134end with fstype kinakaze\n"
        );
        assert_eq!(mountinfo(&[]), "");
        assert_eq!(mounts(&[]), "");
    }

    #[test]
    fn device_fields_preserve_large_major_and_minor_values() {
        let major = 0x123456u64;
        let minor = 0xabcdefu64;
        let encoded = (minor & 0xff)
            | ((major & 0xfff) << 8)
            | ((minor & !0xff) << 12)
            | ((major & !0xfff) << 32);
        assert_eq!(device_numbers(encoded), (major, minor));
    }

    #[test]
    fn native_root_record_uses_real_volume_device_and_backing_directory() {
        let root = crate::system_root().unwrap();
        let record = host_record(&root, 1, 1, "/".into(), 0).unwrap();
        let metadata = crate::fs::stat("/").unwrap();
        assert_eq!(record.device, metadata.st_dev);
        assert_eq!(record.filesystem, "kinakaze");
        assert_eq!(record.target, "/");
        assert!(record.root.starts_with('/'));
        assert!(!record.source.is_empty());
        assert!(["ro", "rw"].contains(&record.options.as_str()));
        assert_eq!(record.options, record.super_options);
    }

    #[test]
    fn native_proc_outputs_track_bind_stack_and_source_overmount() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = std::env::temp_dir().join(format!(
            "kinakaze-mount-report-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&fixture).unwrap();
        struct Fixture {
            root: std::path::PathBuf,
            mounts: Vec<(String, u64)>,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                for (target, id) in self.mounts.iter().rev() {
                    let owned = crate::mount::report_snapshot().ok().is_some_and(|points| {
                        points
                            .iter()
                            .rev()
                            .find(|point| point.target == *target)
                            .is_some_and(|point| point.id == *id)
                    });
                    if !owned || crate::mount::unmount(target, 0).is_err() {
                        return;
                    }
                }
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
        let mut fixture = Fixture {
            root: fixture,
            mounts: Vec::new(),
        };
        for name in ["source tree", "replacement", "target space", "second"] {
            std::fs::create_dir(fixture.root.join(name)).unwrap();
        }
        let source = crate::path::to_guest_path(&fixture.root.join("source tree"));
        let replacement = crate::path::to_guest_path(&fixture.root.join("replacement"));
        let target = crate::path::to_guest_path(&fixture.root.join("target space"));
        let second = crate::path::to_guest_path(&fixture.root.join("second"));
        let pid = crate::job::process_id();
        let before = records(pid).unwrap();
        let expected = host_record(
            &fixture.root.join("source tree"),
            0,
            0,
            target.clone(),
            crate::mount::MS_BIND,
        )
        .unwrap();
        let mut bind = |source: &str, target: &str| {
            crate::mount::bind(source, target, crate::mount::MS_BIND).unwrap();
            let id = crate::mount::report_snapshot()
                .unwrap()
                .iter()
                .rev()
                .find(|point| point.target == target)
                .unwrap()
                .id;
            fixture.mounts.push((target.into(), id));
            id
        };
        let first = bind(&source, &target);
        let source_cover = bind(&replacement, &source);
        let top = bind(&second, &target);
        let after = records(pid).unwrap();
        assert_eq!(after.len(), before.len() + 3);
        let copied = after.iter().find(|record| record.id == first).unwrap();
        assert_eq!(
            copied.root, expected.root,
            "source overmount must not retarget an existing bind"
        );
        assert_eq!(copied.device, expected.device);
        assert_eq!(
            after.iter().find(|record| record.id == top).unwrap().parent,
            first
        );
        assert_ne!(
            after
                .iter()
                .find(|record| record.id == source_cover)
                .unwrap()
                .root,
            copied.root
        );
        let info = mountinfo(&after).into_bytes();
        assert_eq!(
            super::super::read_file("/proc/self/mountinfo").unwrap(),
            info
        );
        assert_eq!(
            super::super::read_file(&format!("/proc/{pid}/mountinfo")).unwrap(),
            info
        );
        let table = mounts(&after).into_bytes();
        assert_eq!(super::super::read_file("/proc/mounts").unwrap(), table);
        assert_eq!(super::super::read_file("/proc/self/mounts").unwrap(), table);
        assert_eq!(
            super::super::read_file(&format!("/proc/{pid}/mounts")).unwrap(),
            table
        );
        assert_eq!(
            super::super::read_file("/proc/self/mountstats").unwrap(),
            mountstats(&after).into_bytes()
        );
        assert!(
            String::from_utf8(info)
                .unwrap()
                .contains("target\\040space")
        );
        crate::mount::unmount(&target, 0).unwrap();
        fixture.mounts.pop();
        let popped = records(pid).unwrap();
        assert!(popped.iter().any(|record| record.id == first));
        assert!(!popped.iter().any(|record| record.id == top));
    }

    #[test]
    fn remote_root_is_not_substituted_with_the_reader_root() {
        let own = crate::job::process_id();
        let other = if own == u32::MAX { 1 } else { own + 1 };
        assert_eq!(records(other), Err(crate::EOPNOTSUPP));
    }
}
