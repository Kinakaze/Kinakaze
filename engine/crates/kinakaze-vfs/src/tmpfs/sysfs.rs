//! Sysfs superblocks bound to a network namespace, backed by native topology
//! and the same network objects used by rtnetlink.
use super::*;
use std::collections::BTreeSet;
#[derive(Clone)]
pub(super) struct Instance {
    pub(super) cgroup: bool,
    netns: u64,
    owner: u64,
    pub(super) paths: BTreeMap<String, u64>,
    buffers: BTreeMap<u64, (u64, String)>,
}
impl Instance {
    pub(super) fn buffers_invalidate(&mut self, id: u64) {
        self.buffers.remove(&id);
    }
    pub(super) fn encode(value: &Option<Self>, b: &mut Vec<u8>) {
        word(b, u64::from(value.is_some()));
        let Some(s) = value else { return };
        word(b, u64::from(s.cgroup));
        word(b, s.netns);
        word(b, s.owner);
        word(b, s.paths.len() as u64);
        for (path, id) in &s.paths {
            bytes(b, path.as_bytes());
            word(b, *id);
        }
        word(b, s.buffers.len() as u64);
        for (id, (event, text)) in &s.buffers {
            word(b, *id);
            word(b, *event);
            bytes(b, text.as_bytes());
        }
    }
    pub(super) fn decode(r: &mut Reader<'_>) -> Result<Option<Self>, i32> {
        match r.word()? {
            0 => return Ok(None),
            1 => (),
            _ => return Err(EIO),
        }
        let mut s = Self {
            cgroup: r.word()? != 0,
            netns: r.word()?,
            owner: r.word()?,
            paths: BTreeMap::new(),
            buffers: BTreeMap::new(),
        };
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            s.paths.insert(r.text()?, r.word()?);
        }
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            s.buffers.insert(r.word()?, (r.word()?, r.text()?));
        }
        Ok(Some(s))
    }
}
fn catalog() -> Result<Arc<Store>, i32> {
    static STORE: std::sync::OnceLock<Arc<Store>> = std::sync::OnceLock::new();
    if let Some(s) = STORE.get() {
        return Ok(s.clone());
    }
    let s = Arc::new(Store::user_object(u64::MAX - 25, true)?);
    let _ = STORE.set(s.clone());
    Ok(s)
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    reconfigure("", options)?;
    let netns = namespaces::current_id(namespaces::NET)?;
    let owner = namespaces::owner(namespaces::NET, netns)?;
    if !user_namespace::capable(owner, 21) {
        return Err(EPERM);
    }
    catalog()?.update(|old| {
        if old.len() % 16 != 0 {
            return Err(EIO);
        }
        let mut entries = Vec::new();
        for row in old.chunks_exact(16) {
            let ns = u64::from_le_bytes(row[..8].try_into().unwrap());
            let id = u64::from_le_bytes(row[8..].try_into().unwrap());
            if Store::user_object(id, false).is_ok() {
                entries.extend_from_slice(row);
                if ns == netns {
                    let _ = volume(id)?;
                    return Ok((old.to_vec(), format!("tmpfs:{id}:1")));
                }
            }
        }
        let source = super::prepare_unkept("size=4k,nr_inodes=1000000,mode=0755")?;
        let (id, _) = parse(&source)?;
        volume(id)?.change(|s| {
            s.sys = Some(Instance {
                cgroup: false,
                netns,
                owner,
                paths: BTreeMap::new(),
                buffers: BTreeMap::new(),
            });
            refresh(s)?;
            Ok(())
        })?;
        start_keeper(id)?;
        word(&mut entries, netns);
        word(&mut entries, id);
        Ok((entries, source))
    })
}
pub(crate) fn reconfigure(_source: &str, options: &str) -> Result<(), i32> {
    if options.is_empty() {
        Ok(())
    } else {
        Err(EINVAL)
    }
}
pub(super) fn keep_catalog() -> Result<(), i32> {
    let _ = catalog()?;
    Ok(())
}
#[derive(Clone)]
pub(super) struct Attribute {
    pub(super) generation: u64,
    pub(super) mode: u32,
    text: String,
}
pub(super) fn put(
    tree: &mut BTreeMap<String, Attribute>,
    path: &str,
    mode: u32,
    text: impl Into<String>,
) {
    let mut parent = path;
    while let Some((p, _)) = parent.rsplit_once('/') {
        tree.entry(p.into()).or_insert(Attribute {
            generation: 0,
            mode: S_IFDIR | 0o755,
            text: String::new(),
        });
        parent = p;
    }
    tree.insert(
        path.into(),
        Attribute {
            generation: 0,
            mode,
            text: text.into(),
        },
    );
}
fn ro(tree: &mut BTreeMap<String, Attribute>, path: &str, text: impl Into<String>) {
    put(tree, path, S_IFREG | 0o444, text);
}
fn dir(tree: &mut BTreeMap<String, Attribute>, path: &str) {
    put(tree, path, S_IFDIR | 0o755, "");
}
fn link(tree: &mut BTreeMap<String, Attribute>, path: &str, target: impl Into<String>) {
    put(tree, path, S_IFLNK | 0o777, target);
}
fn topology() -> Result<(BTreeSet<u32>, BTreeSet<u32>), i32> {
    static TOPOLOGY: std::sync::OnceLock<(BTreeSet<u32>, BTreeSet<u32>)> =
        std::sync::OnceLock::new();
    if let Some(t) = TOPOLOGY.get() {
        return Ok(t.clone());
    }
    let t = crate::cgroup::fixed_defaults::topology()?;
    let _ = TOPOLOGY.set(t.clone());
    Ok(t)
}
fn tree(netns: u64) -> Result<BTreeMap<String, Attribute>, i32> {
    let mut out = BTreeMap::new();
    for p in [
        "block",
        "bus",
        "class",
        "dev/char",
        "dev/block",
        "devices/system",
        "devices/virtual",
        "firmware",
        "fs/cgroup",
        "kernel",
        "module",
        "power",
    ] {
        dir(&mut out, p);
    }
    let (cpus, nodes) = topology()?;
    let cpu_list = crate::cgroup::fixed_defaults::format_list(&cpus);
    let node_list = crate::cgroup::fixed_defaults::format_list(&nodes);
    for name in ["online", "possible", "present"] {
        ro(
            &mut out,
            &format!("devices/system/cpu/{name}"),
            cpu_list.clone(),
        );
    }
    ro(&mut out, "devices/system/cpu/offline", "\n");
    ro(
        &mut out,
        "devices/system/cpu/kernel_max",
        format!("{}\n", cpus.last().copied().ok_or(EIO)?),
    );
    for cpu in cpus {
        let path = format!("devices/system/cpu/cpu{cpu}");
        dir(&mut out, &path);
        ro(&mut out, &format!("{path}/online"), "1\n");
        link(
            &mut out,
            &format!("{path}/subsystem"),
            "../../../../bus/cpu",
        );
        link(
            &mut out,
            &format!("bus/cpu/devices/cpu{cpu}"),
            format!("../../../devices/system/cpu/cpu{cpu}"),
        );
    }
    dir(&mut out, "bus/cpu/drivers");
    for name in [
        "online",
        "possible",
        "has_cpu",
        "has_memory",
        "has_normal_memory",
    ] {
        ro(
            &mut out,
            &format!("devices/system/node/{name}"),
            node_list.clone(),
        );
    }
    for node in nodes {
        let path = format!("devices/system/node/node{node}");
        dir(&mut out, &path);
        let cpus = node_cpus(node)?;
        ro(
            &mut out,
            &format!("{path}/cpulist"),
            crate::cgroup::fixed_defaults::format_list(&cpus),
        );
        for cpu in cpus {
            link(
                &mut out,
                &format!("{path}/cpu{cpu}"),
                format!("../../cpu/cpu{cpu}"),
            );
            link(
                &mut out,
                &format!("devices/system/cpu/cpu{cpu}/node{node}"),
                format!("../../node/node{node}"),
            );
        }
    }
    // Linux huge-page promotion and kernel module loading do not run in this host.
    ro(
        &mut out,
        "kernel/mm/transparent_hugepage/enabled",
        "always madvise [never]\n",
    );
    ro(
        &mut out,
        "kernel/mm/transparent_hugepage/defrag",
        "always defer defer+madvise madvise [never]\n",
    );
    for (name, major, minor, class) in [
        ("null", 1, 3, "mem"),
        ("zero", 1, 5, "mem"),
        ("full", 1, 7, "mem"),
        ("random", 1, 8, "mem"),
        ("urandom", 1, 9, "mem"),
        ("tty", 5, 0, "tty"),
        ("ptmx", 5, 2, "tty"),
    ] {
        let path = format!("devices/virtual/{class}/{name}");
        ro(
            &mut out,
            &format!("{path}/dev"),
            format!("{major}:{minor}\n"),
        );
        ro(
            &mut out,
            &format!("{path}/uevent"),
            format!("MAJOR={major}\nMINOR={minor}\nDEVNAME={name}\n"),
        );
        dir(&mut out, &format!("class/{class}"));
        link(
            &mut out,
            &format!("{path}/subsystem"),
            format!("../../../../class/{class}"),
        );
        link(
            &mut out,
            &format!("class/{class}/{name}"),
            format!("../../{path}"),
        );
        link(
            &mut out,
            &format!("dev/char/{major}:{minor}"),
            format!("../../{path}"),
        );
    }
    dir(&mut out, "class/net");
    dir(&mut out, "devices/virtual/net");
    for interface in crate::netlink::network_interfaces(netns)? {
        let device = &interface.link;
        let base = format!("devices/virtual/net/{}", device.name);
        let mac = |b: &[u8]| {
            format!(
                "{}\n",
                b.iter()
                    .map(|v| format!("{v:02x}"))
                    .collect::<Vec<_>>()
                    .join(":")
            )
        };
        let is_loop = device.kind == "loopback";
        let alias = device
            .attributes
            .iter()
            .find(|a| a.kind == 20)
            .map(|a| {
                String::from_utf8_lossy(&a.value)
                    .trim_end_matches('\0')
                    .to_owned()
            })
            .unwrap_or_default();
        for (name, text) in [
            ("ifindex", format!("{}\n", device.index)),
            (
                "iflink",
                format!(
                    "{}\n",
                    if device.peer != 0 {
                        device.peer
                    } else {
                        device.index
                    }
                ),
            ),
            ("type", format!("{}\n", interface.hardware_type)),
            ("flags", format!("0x{:x}\n", device.flags)),
            ("mtu", format!("{}\n", device.mtu)),
            ("address", mac(&device.address)),
            ("broadcast", mac(&device.broadcast)),
            ("addr_len", format!("{}\n", device.address.len())),
            ("operstate", format!("{}\n", interface.operstate_name())),
            (
                "carrier",
                format!("{}\n", u8::from(interface.operstate == 6)),
            ),
            ("ifalias", format!("{alias}\n")),
            (
                "uevent",
                format!("INTERFACE={}\nIFINDEX={}\n", device.name, device.index),
            ),
        ] {
            put(
                &mut out,
                &format!("{base}/{name}"),
                S_IFREG
                    | if !interface.host
                        && !is_loop
                        && matches!(name, "mtu" | "address" | "ifalias")
                    {
                        0o644
                    } else {
                        0o444
                    },
                text,
            );
        }
        if let Some(stats) = &interface.stats {
            dir(&mut out, &format!("{base}/statistics"));
            for (name, value) in stats.fields() {
                ro(
                    &mut out,
                    &format!("{base}/statistics/{name}"),
                    format!("{value}\n"),
                );
            }
        }
        link(
            &mut out,
            &format!("class/net/{}", device.name),
            format!("../../{base}"),
        );
        link(
            &mut out,
            &format!("{base}/subsystem"),
            "../../../../class/net",
        );
        if device.master != 0 {
            if let Some(master) = crate::route_state::snapshot_for(netns)?
                .links
                .iter()
                .find(|l| l.index == device.master)
            {
                link(
                    &mut out,
                    &format!("{base}/master"),
                    format!("../{}", master.name),
                );
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_operation_refreshes_network_state_without_a_volume_publication() {
        let _serial = crate::procfs::sysctl_test_lock();
        let _network = crate::usernet::scope(1);
        let source = super::super::prepare_unkept("size=4k,nr_inodes=1000000").unwrap();
        let volume = super::super::volume(super::super::parse(&source).unwrap().0).unwrap();
        let id = 0x6000_0000 + u32::try_from(crate::mount::shared::next_group().unwrap()).unwrap();
        let name = format!("obs{}", std::process::id());
        struct LinkGuard(u32);
        impl Drop for LinkGuard {
            fn drop(&mut self) {
                let _ = crate::route_state::transaction(|s| {
                    s.links.retain(|l| l.index != self.0);
                    Ok::<_, i32>(())
                });
            }
        }
        crate::route_state::transaction(|s| {
            s.links.push(crate::route_state::Link {
                index: id,
                name: name.clone(),
                kind: "dummy".into(),
                flags: 1,
                mtu: 1500,
                address: vec![2, 0, 0, 0, 0, 1],
                broadcast: vec![255; 6],
                master: 0,
                peer: 0,
                attributes: vec![],
            });
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        let _link = LinkGuard(id);
        volume
            .change(|s| {
                s.sys = Some(Instance {
                    cgroup: false,
                    netns: 1,
                    owner: 1,
                    paths: BTreeMap::new(),
                    buffers: BTreeMap::new(),
                });
                refresh(s)
            })
            .unwrap();
        let path = format!("devices/virtual/net/{name}/mtu");
        let node;
        {
            let _scope = Observation::enter();
            let first = volume.snapshot().unwrap();
            node = first.sys.as_ref().unwrap().paths[&path];
            let publication = volume.meta.revision();
            assert_eq!(first.nodes[&node].target, "1500\n");
            crate::route_state::transaction(|s| {
                s.links.iter_mut().find(|l| l.index == id).unwrap().mtu = 1400;
                Ok::<_, i32>(())
            })
            .unwrap()
            .unwrap();
            assert_eq!(volume.meta.revision(), publication);
            assert!(Arc::ptr_eq(&first, &volume.snapshot().unwrap()));
        }
        let _scope = Observation::enter();
        let next = volume.snapshot().unwrap();
        assert_eq!(next.sys.as_ref().unwrap().paths[&path], node);
        assert_eq!(next.nodes[&node].target, "1400\n");
    }

    #[test]
    fn native_interfaces_expose_readonly_counters_and_configuration() {
        let _serial = crate::procfs::sysctl_test_lock();
        let tree = super::tree(1).unwrap();
        for interface in crate::hostnet::interfaces(1).unwrap() {
            let base = format!("devices/virtual/net/{}", interface.link.name);
            for field in [
                "mtu",
                "address",
                "ifalias",
                "statistics/rx_bytes",
                "statistics/tx_bytes",
            ] {
                let node = tree.get(&format!("{base}/{field}")).unwrap();
                assert_eq!(node.mode & 0o222, 0, "{field}");
            }
        }
    }
}

fn node_cpus(node: u32) -> Result<BTreeSet<u32>, i32> {
    use windows_sys::Win32::System::{
        SystemInformation::GROUP_AFFINITY, Threading::GetNumaNodeProcessorMask2,
    };
    let mut masks = vec![unsafe { std::mem::zeroed::<GROUP_AFFINITY>() }; 64];
    let mut required = 0;
    if unsafe {
        GetNumaNodeProcessorMask2(
            node as u16,
            masks.as_mut_ptr(),
            masks.len() as u16,
            &mut required,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let mut cpus = BTreeSet::new();
    for m in masks.into_iter().take(required as usize) {
        for bit in 0..64 {
            if m.Mask & (1usize << bit) != 0 {
                cpus.insert(m.Group as u32 * 64 + bit);
            }
        }
    }
    Ok(cpus)
}
pub(super) fn refresh(s: &mut State) -> Result<(), i32> {
    let Some(instance) = s.sys.as_ref() else {
        return Ok(());
    };
    let netns = instance.netns;
    let wanted = if instance.cgroup {
        super::cgroupfs::tree()?
    } else {
        tree(netns)?
    };
    let mut instance = s.sys.take().unwrap();
    let old = std::mem::take(&mut instance.paths);
    let mut paths = BTreeMap::new();
    paths.insert(String::new(), 1);
    for (path, attribute) in wanted {
        let (parent, basename) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        let parent = *paths.get(parent).ok_or(EIO)?;
        let id = if let Some(id) = old.get(&path).copied().filter(|id| {
            s.nodes
                .get(id)
                .is_some_and(|n| n.device == attribute.generation)
        }) {
            id
        } else {
            let id = s.next;
            s.next = s.next.checked_add(1).ok_or(ENOSPC)?;
            s.nodes.insert(id, Node::new(attribute.mode, parent));
            id
        };
        let n = s.nodes.get_mut(&id).ok_or(EIO)?;
        n.device = attribute.generation;
        n.parent = parent;
        n.uid = 0;
        n.gid = 0;
        n.mode = attribute.mode;
        // cgroup enumeration publishes only topology. Preserve any last sampled
        // value/event sequence; read/poll refresh just the requested attribute.
        if !instance.cgroup {
            if n.target != attribute.text {
                n.mtime = now();
            }
            n.target = attribute.text;
        }
        n.size = if n.mode & S_IFMT == S_IFREG {
            if instance.cgroup { 0 } else { PAGE }
        } else if n.mode & S_IFMT == S_IFLNK {
            n.target.len() as u64
        } else {
            0
        };
        n.links = if n.mode & S_IFMT == S_IFDIR { 2 } else { 1 };
        n.children.clear();
        s.nodes
            .get_mut(&parent)
            .ok_or(EIO)?
            .children
            .insert(basename.into(), id);
        paths.insert(path, id);
    }
    let root = s.nodes.get_mut(&1).ok_or(EIO)?;
    root.mode = S_IFDIR | 0o755;
    root.uid = 0;
    root.gid = 0;
    root.size = 0;
    root.children.retain(|name, _| paths.contains_key(name));
    for (path, id) in old {
        if paths.get(&path) != Some(&id) {
            if let Some(n) = s.nodes.get_mut(&id) {
                n.links = 0;
                n.children.clear();
            }
        }
    }
    let counts: Vec<_> = s
        .nodes
        .iter()
        .filter(|(_, n)| n.mode & S_IFMT == S_IFDIR && n.links != 0)
        .map(|(id, n)| {
            (
                *id,
                2 + n
                    .children
                    .values()
                    .filter(|id| s.nodes.get(id).is_some_and(|n| n.mode & S_IFMT == S_IFDIR))
                    .count() as u64,
            )
        })
        .collect();
    for (id, count) in counts {
        s.nodes.get_mut(&id).unwrap().links = count;
    }
    instance
        .buffers
        .retain(|id, _| Store::user_object(*id, false).is_ok());
    paths.remove("");
    instance.paths = paths;
    s.sys = Some(instance);
    Ok(())
}
pub(super) fn read(
    s: &mut State,
    node: u64,
    description: u64,
    buffer: &mut [u8],
    offset: u64,
) -> Result<usize, i32> {
    let n = s.nodes.get(&node).ok_or(ENOENT)?;
    if n.links == 0 {
        return Err(ENODEV);
    }
    if n.mode & S_IFMT == S_IFDIR {
        return Err(EISDIR);
    }
    if n.mode & S_IFMT != S_IFREG {
        return Err(EINVAL);
    }
    let instance = s.sys.as_ref().ok_or(EIO)?;
    if offset == 0 || !instance.buffers.contains_key(&description) {
        if instance.cgroup {
            super::cgroupfs::refresh_attribute(s, node)?;
        }
        let n = s.nodes.get(&node).ok_or(ENOENT)?;
        let buffers = &mut s.sys.as_mut().ok_or(EIO)?.buffers;
        buffers.insert(description, (n.mtime, n.target.clone()));
    }
    let buffers = &s.sys.as_ref().ok_or(EIO)?.buffers;
    let text = &buffers.get(&description).ok_or(EIO)?.1;
    let at = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(text.len());
    let count = buffer.len().min(text.len() - at);
    buffer[..count].copy_from_slice(&text.as_bytes()[at..at + count]);
    Ok(count)
}
pub fn poll(fd: i32) -> Result<u32, i32> {
    let (d, l, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 {
        return Err(EBADF);
    }
    volume(l.volume)?.change(|s| {
        if s.sys.as_ref().is_some_and(|instance| instance.cgroup)
            && s.nodes.get(&l.node).is_some_and(|n| n.links != 0)
        {
            match super::cgroupfs::refresh_attribute(s, l.node) {
                Ok(()) => (),
                Err(ENODEV) => s.nodes.get_mut(&l.node).ok_or(ENOENT)?.links = 0,
                Err(error) => return Err(error),
            }
        }
        let n = s.nodes.get(&l.node).ok_or(ENOENT)?;
        let seen = s
            .sys
            .as_ref()
            .ok_or(EINVAL)?
            .buffers
            .get(&d.id())
            .map(|b| b.0);
        Ok(0x145
            | if n.links == 0 || seen != Some(n.mtime) {
                0xa
            } else {
                0
            })
    })
}
pub(super) fn writable(s: &State, node: u64) -> Result<(), i32> {
    let n = s.nodes.get(&node).ok_or(ENOENT)?;
    if n.links == 0 {
        return Err(ENODEV);
    }
    if n.mode & 0o222 == 0 {
        return Err(EACCES);
    }
    Ok(())
}
pub(super) fn write(s: &mut State, node: u64, buffer: &[u8]) -> Result<usize, i32> {
    writable(s, node)?;
    let instance = s.sys.as_ref().ok_or(EIO)?;
    if instance.cgroup {
        let path = super::cgroupfs::path(s, node)?;
        if buffer.len() > PAGE as usize {
            return Err(7);
        }
        crate::cgroup::write_file(&path, buffer)?;
        refresh(s)?;
        return Ok(buffer.len());
    }
    if !user_namespace::capable(instance.owner, 12) {
        return Err(EPERM);
    }
    if buffer.is_empty() {
        return Ok(0);
    }
    if buffer.len() > PAGE as usize {
        return Err(7);
    }
    let path = instance
        .paths
        .iter()
        .find(|(_, id)| **id == node)
        .map(|(p, _)| p.as_str())
        .ok_or(ENODEV)?;
    let rest = path
        .strip_prefix("devices/virtual/net/")
        .ok_or(EOPNOTSUPP)?;
    let (name, attribute) = rest.split_once('/').ok_or(EOPNOTSUPP)?;
    let text = std::str::from_utf8(buffer)
        .map_err(|_| EINVAL)?
        .trim_end_matches(['\n', '\0']);
    crate::route_state::transaction_for(instance.netns, |network| {
        let link = network
            .links
            .iter_mut()
            .find(|l| l.name == name)
            .ok_or(ENODEV)?;
        match attribute {
            "mtu" => {
                let mtu = text.parse::<u32>().map_err(|_| EINVAL)?;
                if !(68..=65535).contains(&mtu) {
                    return Err(EINVAL);
                }
                link.mtu = mtu;
            }
            "address" => {
                let mac = text
                    .split(':')
                    .map(|p| {
                        if p.len() == 2 {
                            u8::from_str_radix(p, 16).map_err(|_| EINVAL)
                        } else {
                            Err(EINVAL)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if mac.len() != 6 || mac[0] & 1 != 0 || mac.iter().all(|v| *v == 0) {
                    return Err(EINVAL);
                }
                link.address = mac;
            }
            "ifalias" => {
                if text.len() > 255 || text.contains('\0') {
                    return Err(EINVAL);
                }
                link.attributes.retain(|a| a.kind != 20);
                link.attributes.push(crate::route_state::Attribute {
                    kind: 20,
                    value: text.as_bytes().to_vec(),
                });
            }
            _ => return Err(EACCES),
        }
        Ok(())
    })??;
    refresh(s)?;
    Ok(buffer.len())
}

pub(super) fn retain_namespace(instance: &Instance) -> Result<Arc<Store>, i32> {
    if instance.cgroup {
        return crate::cgroup::shared::store();
    }
    keep_catalog()?;
    namespaces::pin_network(instance.netns)
}

pub(super) fn cgroup_instance() -> Instance {
    Instance {
        cgroup: true,
        netns: 0,
        owner: 1,
        paths: BTreeMap::new(),
        buffers: BTreeMap::new(),
    }
}
