//! A transferred description retains the merged inode, directory cursor and
//! mount policy. Native handles pin deleted/renamed inodes during the handoff.
use super::*;
use crate::state_codec::{Reader, bytes, word};

pub(crate) fn export_rights(
    description: &Description,
    mut duplicate: impl FnMut(u64) -> Result<u64, i32>,
) -> Result<Vec<u8>, i32> {
    let d = description;
    let mut out = Vec::new();
    let policy = d.location.policy.as_ref();
    for value in [
        d.inode,
        policy.map_or(0, |p| p.namespace),
        policy.map_or(0, |p| p.id),
        policy.map_or(0, |p| p.flags()),
    ] {
        word(&mut out, value);
    }
    for object in [
        d.writer.as_ref().and_then(|w| w.mount.as_ref()),
        d.writer.as_ref().map(|w| &w.superblock),
        d.anchor.as_ref().map(|a| &a.pin),
    ] {
        word(
            &mut out,
            match object {
                Some(o) => duplicate(o.raw() as u64)?,
                None => 0,
            },
        );
    }
    for text in [
        d.anchor.as_ref().map_or("", |a| a.prefix.as_str()),
        &d.location.instance.source,
        &d.location.guest,
        &d.location.components.join("/"),
        d.directory.as_ref().map_or("", |d| d.name.as_str()),
    ] {
        bytes(&mut out, text.as_bytes());
    }
    let node = d.location.node.as_ref().ok_or(EIO)?;
    word(&mut out, node.entries.len() as u64);
    for entry in &node.entries {
        word(
            &mut out,
            (entry.layer as u64) | (u64::from(entry.indexed) << 63),
        );
        word(&mut out, duplicate(entry.object.raw() as u64)?);
    }
    Ok(out)
}

pub(crate) fn import_rights(
    entry: crate::FdEntry,
    metadata: &[u8],
    mut duplicate: impl FnMut(u64) -> Result<Object, i32>,
) -> Result<(), i32> {
    let mut input = Reader(metadata);
    let inode = input.word()?;
    let namespace = input.word()?;
    let mount_id = input.word()?;
    let flags = input.word()?;
    let mount_raw = input.word()?;
    let super_raw = input.word()?;
    let anchor_raw = input.word()?;
    let prefix = input.text()?;
    let source = input.text()?;
    let guest = input.text()?;
    let components: Vec<String> = input
        .text()?
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    for c in &components {
        super::super::validate_component(c)?;
    }
    let directory_name = input.text()?;
    // The sender remains pinned until the keeper has opened these layer roots;
    // the keeper remains pinned until the receiving process has done the same.
    let instance = instance(&source)?;
    let policy = if mount_id == 0 {
        None
    } else {
        Some(crate::mount::policy::get(namespace, mount_id, flags)?)
    };
    let writer = if super_raw == 0 {
        None
    } else {
        Some(Writer {
            mount: if mount_raw == 0 {
                None
            } else {
                Some(Arc::new(duplicate(mount_raw)?))
            },
            superblock: Arc::new(duplicate(super_raw)?),
        })
    };
    let anchor = if anchor_raw == 0 {
        None
    } else {
        Some(crate::mount::api::DirectoryAnchor {
            pin: Arc::new(duplicate(anchor_raw)?),
            prefix,
        })
    };
    let count = input.word()?;
    if count == 0 || count > 501 {
        return Err(EIO);
    }
    let mut backing = Vec::new();
    for _ in 0..count {
        let layer_flags = input.word()?;
        let layer = (layer_flags & !(1 << 63)) as usize;
        if layer >= instance.root.context.as_ref().ok_or(EIO)?.roots.len() {
            return Err(EIO);
        }
        let object = duplicate(input.word()?)?;
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        let mut value = Backing::from_object(object, instance.root.namespace, layer)?;
        value.indexed = layer_flags >> 63 != 0;
        backing.push(value);
    }
    input.end()?;
    let node = Node {
        entries: backing,
        namespace: instance.root.namespace,
        context: instance.root.context.clone(),
    };
    let directory = if directory_name.is_empty() {
        None
    } else {
        Some(Arc::new(Directory::open(&directory_name, false)?))
    };
    register(
        entry,
        Description {
            location: Location {
                instance,
                policy,
                guest,
                components,
                node: Some(node),
            },
            inode,
            directory,
            writer,
            anchor,
        },
    )
}
