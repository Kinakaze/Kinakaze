//! Socket names are tmpfs inodes. A shared-object lease pins the inode across
//! unlink, fork and exec without storing any tmpfs data in a native disk file.
use super::*;
use crate::fs::object::Object;

pub(crate) struct Inode {
    pub stat: Stat,
    pub pin: Object,
    volume: u64,
    parent: u64,
    name: String,
    node: u64,
    committed: bool,
}

pub(crate) fn bind(path: &str) -> Result<Option<Inode>, i32> {
    let Some(l) = location(path)? else {
        return Ok(None);
    };
    l.writable()?;
    let volume = volume(l.volume)?;
    let lease = shared::new_object()?;
    let pin = lease.pin()?;
    let (node, parent, name, stat) = volume.change(|s| {
        let (parent, name) = s.parent(l.node, &l.tail)?;
        let mut n = Node::new(S_IFSOCK | (0o777 & !fs_context::umask()), parent);
        n.opens.push(lease.id());
        let node = s.insert(l.node, &l.tail, n)?;
        Ok((node, parent, name, s.nodes[&node].stat(node, l.volume)))
    })?;
    Ok(Some(Inode {
        stat,
        pin,
        volume: l.volume,
        parent,
        name,
        node,
        committed: false,
    }))
}

pub(crate) fn lookup(path: &str) -> Result<Option<(Stat, Object)>, i32> {
    let Some(l) = resolve_location(path, true, 0)? else {
        return Ok(None);
    };
    let lease = shared::new_object()?;
    let pin = lease.pin()?;
    let stat = volume(l.volume)?.change(|s| {
        let n = s.nodes.get_mut(&l.node).ok_or(ENOENT)?;
        if n.mode & S_IFMT != S_IFSOCK {
            return Err(crate::ECONNREFUSED);
        }
        n.access(2)?;
        n.opens.push(lease.id());
        Ok(n.stat(l.node, l.volume))
    })?;
    Ok(Some((stat, pin)))
}

impl Inode {
    pub fn commit(&mut self) {
        self.committed = true;
    }
    pub fn rollback(&self) -> Result<(), i32> {
        volume(self.volume)?.change(|s| {
            let Some(parent) = s.nodes.get_mut(&self.parent) else {
                return Ok(());
            };
            if parent.children.get(&self.name) != Some(&self.node) {
                return Ok(());
            }
            parent.children.remove(&self.name);
            parent.mtime = now();
            parent.ctime = parent.mtime;
            let node = s.nodes.get_mut(&self.node).ok_or(EIO)?;
            node.links -= 1;
            node.ctime = now();
            Ok(())
        })
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.rollback();
        }
    }
}
