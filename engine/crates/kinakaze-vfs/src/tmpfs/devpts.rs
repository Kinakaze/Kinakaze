//! Independent devpts superblocks on the shared memory inode engine.
//! Only the PTY allocator creates/removes slave dentries; file I/O is handled
//! by tty's cross-process queues, never by tmpfs regular-file pages.
use super::*;
#[derive(Clone)]
pub(super) struct Instance {
    uid: Option<u32>,
    gid: Option<u32>,
    mode: u32,
    ptmxmode: u32,
    max: u32,
    reserved: bool,
    pub terminals: BTreeMap<u32, (u32, u64)>,
}
impl Default for Instance {
    fn default() -> Self {
        Self {
            uid: None,
            gid: None,
            mode: 0o600,
            ptmxmode: 0,
            max: 1_048_576,
            reserved: false,
            terminals: BTreeMap::new(),
        }
    }
}
impl Instance {
    fn configure(&mut self, options: &str) -> Result<(), i32> {
        for option in options.split(',').filter(|s| !s.is_empty()) {
            let (key, value) = option.split_once('=').unwrap_or((option, ""));
            match key {
                "newinstance" if value.is_empty() => (),
                "uid" | "gid" => {
                    let value = value.parse::<u32>().map_err(|_| EINVAL)?;
                    let value = user_namespace::kernel(value, key == "gid")?;
                    if key == "uid" {
                        self.uid = Some(value);
                    } else {
                        self.gid = Some(value);
                    }
                }
                "mode" | "ptmxmode" => {
                    let value = u32::from_str_radix(value, 8).map_err(|_| EINVAL)? & 0o7777;
                    if key == "mode" {
                        self.mode = value;
                    } else {
                        self.ptmxmode = value;
                    }
                }
                "max" => {
                    self.max = value.parse().map_err(|_| EINVAL)?;
                    if self.max > 1_048_576 {
                        return Err(EINVAL);
                    }
                }
                _ => return Err(EINVAL),
            }
        }
        Ok(())
    }
    pub(super) fn options(&self) -> String {
        let mut out = format!(
            "rw,mode={:o},ptmxmode={:o},max={}",
            self.mode, self.ptmxmode, self.max
        );
        if let Some(uid) = self.uid {
            out.push_str(&format!(",uid={}", user_namespace::visible(uid, false)));
        }
        if let Some(gid) = self.gid {
            out.push_str(&format!(",gid={}", user_namespace::visible(gid, true)));
        }
        out
    }
    pub(super) fn encode(value: &Option<Self>, b: &mut Vec<u8>) {
        word(b, u64::from(value.is_some()));
        if let Some(p) = value {
            for v in [
                p.uid.map_or(u64::MAX, u64::from),
                p.gid.map_or(u64::MAX, u64::from),
                p.mode as u64,
                p.ptmxmode as u64,
                p.max as u64,
                u64::from(p.reserved),
                p.terminals.len() as u64,
            ] {
                word(b, v);
            }
            for (number, (internal, node)) in &p.terminals {
                for v in [*number as u64, *internal as u64, *node] {
                    word(b, v);
                }
            }
        }
    }
    pub(super) fn decode(r: &mut Reader<'_>) -> Result<Option<Self>, i32> {
        match r.word()? {
            0 => return Ok(None),
            1 => (),
            _ => return Err(EIO),
        }
        let uid = r.word()?;
        let gid = r.word()?;
        let mut p = Self {
            uid: if uid == u64::MAX {
                None
            } else {
                Some(uid as u32)
            },
            gid: if gid == u64::MAX {
                None
            } else {
                Some(gid as u32)
            },
            mode: r.word()? as u32,
            ptmxmode: r.word()? as u32,
            max: r.word()? as u32,
            reserved: r.word()? != 0,
            terminals: BTreeMap::new(),
        };
        let count = r.word()?;
        if count > 1_048_576 {
            return Err(EIO);
        }
        for _ in 0..count {
            p.terminals
                .insert(r.word()? as u32, (r.word()? as u32, r.word()?));
        }
        Ok(Some(p))
    }
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    let mut p = Instance::default();
    p.configure(options)?;
    p.reserved = mount::namespace_id()? == 1;
    let source = super::prepare_unkept("size=4k,nr_inodes=1048578,mode=0755")?;
    let (id, _) = parse(&source)?;
    volume(id)?.change(|s| {
        let mut ptmx = Node::new(S_IFCHR | p.ptmxmode, 1);
        ptmx.device = (5 << 8) | 2;
        s.nodes.insert(2, ptmx);
        s.nodes
            .get_mut(&1)
            .ok_or(EIO)?
            .children
            .insert("ptmx".into(), 2);
        s.next = 3;
        s.pts = Some(p);
        Ok(())
    })?;
    start_keeper(id)?;
    Ok(source)
}
pub(crate) fn reconfigure(source: &str, options: &str) -> Result<(), i32> {
    let (id, _) = parse(source)?;
    volume(id)?.change(|s| {
        let p = s.pts.as_mut().ok_or(EINVAL)?;
        p.configure(options)?;
        s.nodes.get_mut(&2).ok_or(EIO)?.mode = S_IFCHR | p.ptmxmode;
        Ok(())
    })
}
pub(super) fn collect(s: &mut State) {
    let Some(p) = &mut s.pts else {
        return;
    };
    p.terminals.retain(|number, (internal, node)| {
        let (master, any) = tty::instance_status(*internal);
        if !master {
            s.nodes
                .get_mut(&1)
                .unwrap()
                .children
                .remove(&number.to_string());
            if let Some(n) = s.nodes.get_mut(node) {
                n.links = 0;
            }
        }
        any
    });
}
pub fn owns(path: &str) -> bool {
    location(path).is_ok_and(|l| l.is_some_and(|l| l.flags & mount::MS_DEVPTS != 0))
}
pub(super) fn open_character(l: &Location, n: &Node, flags: i32) -> Result<i32, i32> {
    let (major, minor) = fs::device_numbers(n.device);
    fs::check_character_device_open(flags, major, minor)?;
    let v = volume(l.volume)?;
    let s = State::decode(&v.meta.read()?.1)?;
    if let Some(p) = s.pts {
        if l.node == 2 {
            return allocate(l, flags);
        }
        let (internal, _) = p
            .terminals
            .values()
            .find(|(_, node)| *node == l.node)
            .ok_or(EIO)?;
        if !tty::instance_status(*internal).0 {
            return Err(EIO);
        }
        return tty::open_instance_slave(*internal, flags);
    }
    if n.device == (5 << 8) | 2 {
        let parent = l.path.rsplit_once('/').map_or("", |(p, _)| p);
        let pts = location(&format!("{parent}/pts"))?.ok_or(ENODEV)?;
        if pts.flags & mount::MS_DEVPTS == 0
            || pts.node != 1
            || !pts.tail.trim_matches('/').is_empty()
        {
            return Err(ENODEV);
        }
        return allocate(&pts, flags);
    }
    fs::open_device_value(n.device, n.mode, flags)
}
fn allocate(l: &Location, flags: i32) -> Result<i32, i32> {
    let id = l.volume;
    let v = volume(id)?;
    let mut installed = None;
    let result = v.change(|s| {
        let p = s.pts.as_ref().ok_or(ENODEV)?;
        if p.terminals.len() as u32 >= p.max {
            return Err(EIO);
        }
        let number = (0..1_048_576)
            .find(|n| !p.terminals.contains_key(n))
            .ok_or(EIO)?;
        let c = credentials::filesystem();
        let mut n = Node::new(S_IFCHR | p.mode, 1);
        n.inode_number = number as u64 + 3;
        n.uid = p.uid.unwrap_or(c.uid);
        n.gid = p.gid.unwrap_or(c.gid);
        let major = 136 + (number as u64 >> 8);
        n.device = ((major & 0xfff) << 8) | ((major & !0xfff) << 32) | (number as u64 & 255);
        // The inode generation is unique even when the visible index is reused;
        // retained O_PATH descriptors keep the old, unlinked inode.
        let node = s.next;
        s.next = s.next.checked_add(1).ok_or(ENOSPC)?;
        let (fd, internal) = tty::open_instance_master(
            id,
            node,
            number,
            flags,
            p.reserved,
            (l.namespace, l.mount, l.flags),
        )?;
        s.nodes.insert(node, n);
        s.nodes
            .get_mut(&1)
            .unwrap()
            .children
            .insert(number.to_string(), node);
        s.pts
            .as_mut()
            .unwrap()
            .terminals
            .insert(number, (internal, node));
        installed = Some(fd);
        Ok(fd)
    });
    if result.is_err() {
        if let Some(fd) = installed {
            let _ = crate::close(fd);
        }
    }
    result
}
pub fn fstat(fd: i32) -> Result<Option<Stat>, i32> {
    let Some((id, node, _)) = tty::instance(fd)? else {
        return Ok(None);
    };
    let v = volume(id)?;
    v.change(|s| {
        let node = if tty::pty_side(fd)? == tty::Side::Master {
            2
        } else {
            node
        };
        Ok(Some(s.nodes.get(&node).ok_or(EIO)?.stat(node, id)))
    })
}
pub fn fstatfs(fd: i32) -> Result<Option<Statistics>, i32> {
    let Some((id, node, _)) = tty::instance(fd)? else {
        return Ok(None);
    };
    statistics(&fd_location(fd, id, node)?).map(Some)
}
pub fn fchmod(fd: i32, mode: u32) -> Result<bool, i32> {
    let Some((id, node, _)) = tty::instance(fd)? else {
        return Ok(false);
    };
    let node = if tty::pty_side(fd)? == tty::Side::Master {
        2
    } else {
        node
    };
    change_mode(fd_location(fd, id, node)?, mode)?;
    Ok(true)
}
pub fn fchown(fd: i32, owner: &Ownership) -> Result<bool, i32> {
    let Some((id, node, _)) = tty::instance(fd)? else {
        return Ok(false);
    };
    let node = if tty::pty_side(fd)? == tty::Side::Master {
        2
    } else {
        node
    };
    change_owner(fd_location(fd, id, node)?, owner)?;
    Ok(true)
}
fn fd_location(fd: i32, id: u64, node: u64) -> Result<Location, i32> {
    let (namespace, mount, flags) = tty::instance_policy(fd)?;
    Ok(Location {
        volume: id,
        node,
        namespace,
        mount,
        flags,
        tail: String::new(),
        path: String::new(),
    })
}

pub(crate) fn terminal_path(id: u64, number: u32, side: tty::Side) -> Result<Option<String>, i32> {
    let name = if side == tty::Side::Master {
        "ptmx".into()
    } else {
        number.to_string()
    };
    for p in mount::snapshot_list()? {
        if p.flags & mount::MS_DEVPTS != 0 && parse(&p.source).is_ok_and(|(v, n)| v == id && n == 1)
        {
            let path = format!("{}/{}", p.target.trim_end_matches('/'), name);
            if location(&path)?.is_some_and(|l| l.volume == id) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}
pub(crate) fn open_sibling(path: &str, flags: i32) -> Result<Option<i32>, i32> {
    let parent = path.rsplit_once('/').map_or("", |(p, _)| p);
    let Some(pts) = location(&format!("{parent}/pts"))? else {
        return Ok(None);
    };
    if pts.flags & mount::MS_DEVPTS == 0 || pts.node != 1 || !pts.tail.trim_matches('/').is_empty()
    {
        return Err(ENODEV);
    }
    allocate(&pts, flags).map(Some)
}
