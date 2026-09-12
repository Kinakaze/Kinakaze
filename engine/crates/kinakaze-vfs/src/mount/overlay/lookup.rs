//! Feature-aware component lookup. Redirects are confined to pinned layer roots.
use super::*;

impl Node {
    pub(super) fn feature_child(&self, name: &str, depth: usize) -> Result<Self, i32> {
        validate_component(name)?;
        if !self.is_directory() {
            return Err(ENOTDIR);
        }
        if depth >= 40 {
            return Err(crate::ELOOP);
        }
        let mut found: Vec<Backing> = Vec::new();
        let mut need_data = false;
        for parent in &self.entries {
            let stored = crate::path::escape_component(name);
            let entry = match parent
                .object
                .child(
                    OsStr::new(stored.as_ref()),
                    FILE_READ_ATTRIBUTES | FILE_READ_EA,
                )
                .and_then(|object| Backing::from_object(object, self.namespace, parent.layer))
            {
                Ok(entry) => entry,
                Err(ENOENT) => continue,
                Err(e) => return Err(e),
            };
            if entry.whiteout {
                break;
            }
            if !found.is_empty() && entry.is_directory() != found[0].is_directory() {
                break;
            }
            if entry.metacopy.is_some() {
                if entry.metadata.st_mode & S_IFMT != S_IFREG {
                    return Err(EIO);
                }
                if self.flags() & features::METACOPY == 0
                    && (features::data_count(self.flags()) == 0
                        || !entry
                            .redirect
                            .as_ref()
                            .is_some_and(|path| path.starts_with('/')))
                {
                    return Err(EIO);
                }
                need_data = true;
            }
            if entry.redirect.is_some()
                && (entry.is_directory() && self.flags() & features::FOLLOW != 0
                    || entry.metacopy.is_some()
                        && (self.flags() & features::FOLLOW != 0
                            || features::data_count(self.flags()) != 0))
            {
                let redirect = entry.redirect.as_deref().unwrap();
                if redirect.is_empty() || redirect.contains(['\0', '\\']) {
                    return Err(EIO);
                }
                let data = entry.metacopy.is_some();
                let mut target = self.redirected(parent, entry.layer, redirect, depth + 1, data)?;
                if entry.is_directory() != target.is_directory() {
                    return Err(EIO);
                }
                found.push(entry);
                found.append(&mut target.entries);
                need_data = false;
                break;
            }
            let stop = (!entry.is_directory() && entry.metacopy.is_none()) || entry.opaque;
            if need_data && entry.metacopy.is_none() {
                need_data = false;
            }
            found.push(entry);
            if stop {
                break;
            }
        }
        if found.is_empty() {
            return Err(ENOENT);
        }
        if need_data {
            return Err(EIO);
        }
        let mut node = Self {
            entries: found,
            namespace: self.namespace,
            context: self.context.clone(),
        };
        if node.is_directory()
            && node.backing_layer() == 0
            && node.flags() & features::NFS_EXPORT != 0
        {
            let attrs = unsafe { Attributes::from_handle(node.backing_object().raw(), false)? }
                .snapshot()?;
            if let Some(bytes) = attrs.get(format!("{}origin", self.namespace.prefix()).as_bytes())
            {
                let origin = identity::Identity::decode(bytes)?;
                if let Some(lower) = node.entries.iter().find(|entry| entry.layer != 0) {
                    let lower = Node {
                        entries: vec![lower.clone()],
                        namespace: self.namespace,
                        context: node.context.clone(),
                    };
                    if lower.origin()? != origin {
                        node.entries.truncate(1);
                    }
                }
                let context = node.context.as_ref().ok_or(EIO)?;
                if let Some(index) = &context.index {
                    let root = Node {
                        entries: context.roots.clone(),
                        namespace: node.namespace,
                        context: node.context.clone(),
                    };
                    if let Some(upper) = index.directory_upper(&origin, &root)?
                        && crate::fs::stat_handle(upper.raw(), false)?.st_ino
                            != node.backing_metadata().st_ino
                    {
                        return Err(EIO);
                    }
                }
            }
        }
        if !node.is_directory()
            && node.backing_layer() != 0
            && let Some(index) = node
                .context
                .as_ref()
                .and_then(|context| context.index.as_ref())
            && let Some(object) = index.lookup(&node.origin()?)?
        {
            let mut indexed = Backing::from_object(object, self.namespace, node.backing_layer())?;
            indexed.indexed = true;
            if indexed.metacopy.is_some() {
                node.entries.insert(0, indexed);
            } else {
                node.entries = vec![indexed];
            }
        }
        node.verify_metacopy()?;
        Ok(node)
    }

    fn redirected(
        &self,
        parent: &Backing,
        layer: usize,
        redirect: &str,
        depth: usize,
        data: bool,
    ) -> Result<Node, i32> {
        let context = self.context.as_ref().ok_or(EIO)?;
        let relative = if redirect.starts_with('/') {
            redirect.trim_start_matches('/').to_owned()
        } else {
            let root = context
                .roots
                .iter()
                .find(|root| root.layer == parent.layer)
                .ok_or(EIO)?;
            let root_path = root.object.path()?;
            let path = parent.object.path()?;
            let relative = path.strip_prefix(root_path).map_err(|_| EIO)?;
            let mut names = Vec::new();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    return Err(EIO);
                };
                names.push(crate::path::unescape_path(name.to_str().ok_or(EIO)?).into_owned());
            }
            names.push(redirect.into());
            names.join("/")
        };
        let parts: Vec<_> = relative.split('/').collect();
        for component in &parts {
            validate_component(component).map_err(|_| EIO)?;
        }
        let visible = context.roots.len() - features::data_count(context.flags);
        // Data-only mode does not enable ordinary metacopy/redirect semantics.
        if !data || self.flags() & features::METACOPY != 0 {
            let entries = context.roots[..visible]
                .iter()
                .filter(|entry| entry.layer > layer)
                .cloned()
                .collect::<Vec<_>>();
            if !entries.is_empty() {
                let lookup = || {
                    let mut target = Node {
                        entries,
                        namespace: self.namespace,
                        context: self.context.clone(),
                    };
                    for component in &parts {
                        target = target.feature_child(component, depth)?;
                    }
                    Ok(target)
                };
                match lookup() {
                    Ok(target) => return Ok(target),
                    Err(ENOENT) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        if data && redirect.starts_with('/') {
            for root in &context.roots[visible..] {
                let lookup = || {
                    let mut target = Node {
                        entries: vec![root.clone()],
                        namespace: self.namespace,
                        context: None,
                    };
                    for component in &parts {
                        target = target.child(component)?;
                    }
                    if target.backing_metadata().st_mode & S_IFMT != S_IFREG {
                        return Err(EIO);
                    }
                    Ok(target)
                };
                match lookup() {
                    Ok(target) => return Ok(target),
                    Err(ENOENT) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Err(EIO)
    }
}
