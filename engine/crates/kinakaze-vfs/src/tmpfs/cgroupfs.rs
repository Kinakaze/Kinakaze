//! A mountable cgroup v2 hierarchy, using shared kernfs-style inode descriptions.
use super::sysfs::{Attribute, put};
use super::*;

fn catalog() -> Result<Arc<Store>, i32> {
    static STORE: std::sync::OnceLock<Arc<Store>> = std::sync::OnceLock::new();
    if let Some(s) = STORE.get() {
        return Ok(s.clone());
    }
    let s = Arc::new(Store::user_object(u64::MAX - 27, true)?);
    let _ = STORE.set(s.clone());
    Ok(s)
}
pub(crate) fn reconfigure(_source: &str, options: &str) -> Result<(), i32> {
    if options.is_empty() {
        Ok(())
    } else {
        Err(EINVAL)
    }
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    reconfigure("", options)?;
    let ns = namespaces::current_id(namespaces::CGROUP)?;
    if !user_namespace::capable(namespaces::owner(namespaces::CGROUP, ns)?, 21) {
        return Err(EPERM);
    }
    let root = namespaces::cgroup_root()?;
    let relative = root
        .strip_prefix("/sys/fs/cgroup")
        .ok_or(EIO)?
        .trim_start_matches('/');
    // One unified hierarchy, with the calling cgroup namespace selecting its root.
    catalog()?.update(|old| {
        let existing = old
            .get(..8)
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .filter(|id| Store::user_object(*id, false).is_ok());
        let id = if let Some(id) = existing {
            id
        } else {
            let source = super::prepare_unkept("size=4k,nr_inodes=1000000,mode=0755")?;
            let (id, _) = parse(&source)?;
            volume(id)?.change(|s| {
                s.sys = Some(sysfs::cgroup_instance());
                sysfs::refresh(s)
            })?;
            start_keeper(id)?;
            id
        };
        let v = volume(id)?;
        let node = v.change(|s| s.resolve(1, relative))?;
        Ok((id.to_le_bytes().to_vec(), format!("tmpfs:{id}:{node}")))
    })
}
pub(super) fn tree() -> Result<BTreeMap<String, Attribute>, i32> {
    let _ = catalog()?;
    let mut out = BTreeMap::new();
    for (path, mode, generation) in crate::cgroup::filesystem_tree()? {
        put(&mut out, &path, mode, "");
        out.get_mut(&path).ok_or(EIO)?.generation = generation;
    }
    Ok(out)
}
/// Query only the requested inode. The retained group ID, rather than its path
/// alone, identifies it across deletion/recreation and detached mounts.
pub(super) fn refresh_attribute(s: &mut State, node: u64) -> Result<(), i32> {
    let path = path(s, node)?;
    let generation = s.nodes.get(&node).ok_or(ENODEV)?.device;
    let content = crate::cgroup::read_file_generation(&path, generation)?;
    let text = String::from_utf8(content).map_err(|_| EIO)?;
    let n = s.nodes.get_mut(&node).ok_or(ENODEV)?;
    if n.target != text {
        n.mtime = now().max(n.mtime.saturating_add(1));
        n.target = text;
    }
    Ok(())
}
pub(super) fn path(s: &State, node: u64) -> Result<String, i32> {
    if s.sys.as_ref().is_none_or(|i| !i.cgroup) {
        return Err(EINVAL);
    }
    if s.nodes.get(&node).is_none_or(|n| n.links == 0) {
        return Err(ENODEV);
    }
    if node == 1 {
        return Ok("/sys/fs/cgroup".into());
    }
    let path = s
        .sys
        .as_ref()
        .filter(|i| i.cgroup)
        .ok_or(EINVAL)?
        .paths
        .iter()
        .find(|(_, n)| **n == node)
        .map(|(p, _)| p)
        .ok_or(ENODEV)?;
    Ok(format!("/sys/fs/cgroup/{path}"))
}
pub(super) fn create(s: &mut State, start: u64, tail: &str, mode: u32) -> Result<u64, i32> {
    if mode & S_IFMT != S_IFDIR {
        return Err(EPERM);
    }
    let (parent, name) = s.parent(start, tail)?;
    if s.nodes[&parent].children.contains_key(&name) {
        return Err(EEXIST);
    }
    let path = format!("{}/{name}", path(s, parent)?);
    crate::cgroup::create_directory(&path)?;
    sysfs::refresh(s)?;
    s.resolve(parent, &name)
}
pub(super) fn remove(s: &mut State, start: u64, tail: &str, directory: bool) -> Result<(), i32> {
    let node = s.resolve(start, tail)?;
    if s.nodes[&node].mode & S_IFMT != S_IFDIR {
        return Err(if directory { ENOTDIR } else { EPERM });
    }
    if !directory {
        return Err(EISDIR);
    }
    let (parent, _) = s.parent(start, tail)?;
    s.nodes[&parent].access(3)?;
    crate::cgroup::remove_directory(&path(s, node)?)?;
    sysfs::refresh(s)
}
pub fn descriptor_path(fd: i32) -> Result<String, i32> {
    let (_, l, _, _) = descriptor(fd)?;
    let s = volume(l.volume)?.snapshot()?;
    if s.nodes
        .get(&l.node)
        .is_none_or(|n| n.mode & S_IFMT != S_IFDIR)
    {
        return Err(ENOTDIR);
    }
    path(&s, l.node)
}
pub(crate) fn mount_root(source: &str) -> Result<String, i32> {
    let (id, node) = parse(source)?;
    let path = path(&*volume(id)?.snapshot()?, node)?;
    Ok(path
        .strip_prefix("/sys/fs/cgroup")
        .filter(|s| !s.is_empty())
        .unwrap_or("/")
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_inode_cannot_read_a_group_recreated_after_topology_observation() {
        let _serial = crate::procfs::sysctl_test_lock();
        let name = format!("lazy-generation-{}", shared::next_group().unwrap());
        let group = format!("/sys/fs/cgroup/{name}");
        crate::cgroup::create_directory(&group).unwrap();
        struct Group(String);
        impl Drop for Group {
            fn drop(&mut self) {
                let _ = crate::cgroup::remove_directory(&self.0);
            }
        }
        let _group = Group(group.clone());
        let source = super::super::prepare_unkept("size=4k,nr_inodes=1000000").unwrap();
        let v = volume(parse(&source).unwrap().0).unwrap();
        v.change(|s| {
            s.sys = Some(sysfs::cgroup_instance());
            sysfs::refresh(s)
        })
        .unwrap();
        let mut observed = State::decode(&v.snapshot().unwrap().encode()).unwrap();
        let node = observed.sys.as_ref().unwrap().paths[&format!("{name}/cpu.max")];
        refresh_attribute(&mut observed, node).unwrap();
        assert_eq!(observed.nodes[&node].target, "max 100000\n");

        crate::cgroup::remove_directory(&group).unwrap();
        crate::cgroup::create_directory(&group).unwrap();
        crate::cgroup::write_file(&format!("{group}/cpu.max"), b"50000 100000\n").unwrap();
        // Deliberately keep the old topology: this is the race between resolve
        // and read, not merely the ordinary next-operation topology refresh.
        assert_eq!(refresh_attribute(&mut observed, node), Err(ENODEV));
        assert_eq!(observed.nodes[&node].target, "max 100000\n");
        sysfs::refresh(&mut observed).unwrap();
        let replacement = observed.sys.as_ref().unwrap().paths[&format!("{name}/cpu.max")];
        assert_ne!(node, replacement);
        refresh_attribute(&mut observed, replacement).unwrap();
        assert_eq!(observed.nodes[&replacement].target, "50000 100000\n");
    }
}
