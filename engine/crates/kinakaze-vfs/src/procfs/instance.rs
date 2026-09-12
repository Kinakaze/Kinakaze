//! Independent proc superblocks and descriptor-pinned views.
use crate::mount::{self, policy};
use crate::{EINVAL, EIO, ENOENT, EOPNOTSUPP, EPERM};
use std::cell::RefCell;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct View {
    pub instance: u64,
    pub pidns: u64,
    pub owner: u64,
    pub subset_pid: bool,
    pub namespace: u64,
    pub mount: u64,
    pub flags: u64,
    pub target: String,
}
thread_local! { static ACTIVE: RefCell<Option<View>> = const { RefCell::new(None) }; }
pub(crate) struct Scope(Option<View>);
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|s| *s.borrow_mut() = self.0.take());
    }
}
pub(crate) fn current() -> Option<View> {
    ACTIVE.with(|s| s.borrow().clone())
}
pub(crate) fn pidns() -> u64 {
    current().map_or_else(
        || crate::namespaces::current_id(crate::namespaces::PID).unwrap_or(1),
        |v| v.pidns,
    )
}
pub(crate) fn source(view: &View) -> String {
    format!(
        "procfs:{}:{}:{}:{}/",
        view.instance,
        view.pidns,
        view.owner,
        u8::from(view.subset_pid)
    )
}
pub(crate) fn parse(source: &str) -> Result<(View, &str), i32> {
    let (head, tail) = source
        .strip_prefix("procfs:")
        .ok_or(EINVAL)?
        .split_once('/')
        .ok_or(EINVAL)?;
    let words = head
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| EINVAL)?;
    if words.len() != 4 || words[..3].contains(&0) || words[3] > 1 {
        return Err(EINVAL);
    }
    Ok((
        View {
            instance: words[0],
            pidns: words[1],
            owner: words[2],
            subset_pid: words[3] != 0,
            namespace: 0,
            mount: 0,
            flags: 0,
            target: String::new(),
        },
        tail,
    ))
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    let pidns = crate::namespaces::current_id(crate::namespaces::PID)?;
    let userns = crate::user_namespace::id(crate::job::process_id())?;
    let owner = crate::namespaces::owner(crate::namespaces::PID, pidns)?;
    if !crate::user_namespace::capable(userns, 21) || !crate::user_namespace::capable(owner, 21) {
        return Err(EPERM);
    }
    let mut subset_pid = false;
    for option in options.split(',').filter(|s| !s.is_empty()) {
        match option {
            "subset=pid" => subset_pid = true,
            "subset=all" => subset_pid = false,
            "hidepid=0" | "hidepid=off" => {}
            value if value.starts_with("hidepid=") || value.starts_with("gid=") => {
                return Err(EOPNOTSUPP);
            }
            _ => return Err(EINVAL),
        }
    }
    let object = mount::shared::new_object()?;
    Ok(source(&View {
        instance: object.id(),
        pidns,
        owner: userns,
        subset_pid,
        namespace: 0,
        mount: 0,
        flags: 0,
        target: String::new(),
    }))
}
impl View {
    pub fn device(&self) -> u64 {
        0x7000_0000_0000 | self.instance
    }
    fn policy(&self) -> Result<Arc<policy::Policy>, i32> {
        policy::get(self.namespace, self.mount, self.flags)
    }
    pub fn live_flags(&self) -> Result<u64, i32> {
        Ok(self.policy()?.flags())
    }
    pub(crate) fn pinned_path(&self, path: &str) -> String {
        format!(
            "/proc/.mount/{}/{}/{}/{}/{}/{}/{}/{}/{}",
            self.namespace,
            self.mount,
            self.flags,
            self.instance,
            self.pidns,
            self.owner,
            u8::from(self.subset_pid),
            hex(&self.target),
            path.strip_prefix("/proc")
                .unwrap_or(path)
                .trim_start_matches('/')
        )
    }
}
pub(crate) fn encode(path: &str) -> String {
    current().map_or_else(|| path.to_owned(), |v| v.pinned_path(path))
}
fn decode(path: &str) -> Result<(View, String), i32> {
    let mut parts = path
        .strip_prefix("/proc/.mount/")
        .ok_or(ENOENT)?
        .splitn(9, '/');
    let mut words = [0u64; 7];
    for word in &mut words {
        *word = parts.next().ok_or(EIO)?.parse().map_err(|_| EIO)?;
    }
    if words[..6]
        .iter()
        .enumerate()
        .any(|(i, n)| i != 2 && *n == 0)
        || words[6] > 1
    {
        return Err(EIO);
    }
    let target = unhex(parts.next().ok_or(EIO)?)?;
    let tail = parts.next().unwrap_or("");
    Ok((
        View {
            namespace: words[0],
            mount: words[1],
            flags: words[2],
            instance: words[3],
            pidns: words[4],
            owner: words[5],
            subset_pid: words[6] != 0,
            target,
        },
        format!("/proc/{tail}"),
    ))
}
pub(crate) fn lookup(path: &str) -> Result<Option<(View, String)>, i32> {
    if path.starts_with("/proc/.mount/") {
        return if super::PINNED_PATH.get() {
            decode(path).map(Some)
        } else {
            Ok(None)
        };
    }
    mount::proc_location(path)
}
pub(crate) fn enter(path: &str) -> Result<(String, Scope), i32> {
    if current().is_some()
        && (path == "/proc" || path.starts_with("/proc/"))
        && !path.starts_with("/proc/.mount/")
    {
        let old = current();
        return Ok((path.into(), Scope(old)));
    }
    let (view, path) = match lookup(path)? {
        Some((v, p)) => (Some(v), p),
        None => (None, path.into()),
    };
    let path = mount::normalize(&path)?;
    if let Some(view) = &view {
        // Pin policy before a descriptor can survive unmount or a namespace switch.
        let _ = view.policy()?;
        if view.subset_pid {
            let item = path
                .strip_prefix("/proc/")
                .unwrap_or("")
                .split('/')
                .next()
                .unwrap_or("");
            if !item.is_empty()
                && !matches!(item, "self" | "thread-self")
                && item.parse::<u32>().is_err()
            {
                return Err(ENOENT);
            }
        }
    }
    let old = ACTIVE.with(|s| s.replace(view));
    Ok((path, Scope(old)))
}
pub(crate) fn check_write(path: &str) -> Result<(), i32> {
    let (_, _scope) = enter(path)?;
    if current().is_some_and(|v| v.live_flags().map_or(true, |f| f & 1 != 0)) {
        return Err(crate::EROFS);
    }
    Ok(())
}
pub fn filesystem(path: &str) -> Result<Option<(u64, u64)>, i32> {
    let (_, _scope) = enter(path)?;
    current()
        .map(|v| Ok((v.device(), v.live_flags()?)))
        .transpose()
}

fn hex(text: &str) -> String {
    if text.is_empty() {
        "-".into()
    } else {
        text.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }
}
fn unhex(text: &str) -> Result<String, i32> {
    if text == "-" {
        return Ok(String::new());
    }
    if !text.len().is_multiple_of(2) {
        return Err(EIO);
    }
    String::from_utf8(
        text.as_bytes()
            .chunks_exact(2)
            .map(|p| {
                u8::from_str_radix(std::str::from_utf8(p).map_err(|_| EIO)?, 16).map_err(|_| EIO)
            })
            .collect::<Result<Vec<_>, _>>()?,
    )
    .map_err(|_| EIO)
}
pub(crate) fn display(path: String) -> Result<String, i32> {
    if !path.starts_with("/proc/.mount/") {
        return Ok(path);
    }
    let (view, inside) = decode(&path)?;
    let mut parts: Vec<_> = inside
        .strip_prefix("/proc")
        .ok_or(EIO)?
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    if let Some(pid) = parts.first().and_then(|p| p.parse::<u32>().ok()) {
        if let Some(visible) = kinakaze_runtime::job::namespaces::visible_in(pid, view.pidns) {
            parts[0] = visible.to_string();
        }
    }
    Ok(format!(
        "{}/{}",
        if view.target.is_empty() {
            "/proc"
        } else {
            view.target.trim_end_matches('/')
        },
        parts.join("/")
    )
    .trim_end_matches('/')
    .to_owned())
}
