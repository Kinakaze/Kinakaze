//! Mount-event propagation between peer groups and their downstream slaves.
use super::*;
use std::collections::{HashMap, HashSet};

pub(super) fn change(point: &mut MountPoint, kind: u64) -> Result<(), i32> {
    match kind {
        MS_SHARED => {
            if point.meta.peer == 0 {
                point.meta.peer = shared::next_group()?;
            }
            point.flags = (point.flags & !MS_PROPAGATION)
                | MS_SHARED
                | if point.meta.master != 0 { MS_SLAVE } else { 0 };
        }
        MS_SLAVE => {
            if point.meta.peer != 0 {
                point.meta.master = point.meta.peer;
            }
            point.meta.peer = 0;
            point.flags = (point.flags & !MS_PROPAGATION)
                | if point.meta.master != 0 {
                    MS_SLAVE
                } else {
                    MS_PRIVATE
                };
        }
        MS_PRIVATE | MS_UNBINDABLE => {
            point.meta.peer = 0;
            point.meta.master = 0;
            point.flags = (point.flags & !MS_PROPAGATION) | kind;
        }
        _ => return Err(EINVAL),
    }
    Ok(())
}
struct Table {
    store: Arc<shared::Store>,
    points: Vec<MountPoint>,
    next: u64,
}
fn next(bytes: &[u8]) -> Result<u64, i32> {
    if bytes.is_empty() {
        Ok(FIRST_DYNAMIC_MOUNT_ID)
    } else {
        Ok(u64::from_le_bytes(
            bytes.get(16..24).ok_or(EIO)?.try_into().unwrap(),
        ))
    }
}
pub(super) fn update<T>(
    action: impl FnOnce(&mut Vec<MountPoint>, &mut u64) -> Result<T, i32>,
) -> Result<T, i32> {
    let _guard = shared::topology_guard()?;
    let own = shared::get()?;
    let (_, bytes) = own.read()?;
    let before = decode_table(&bytes)?;
    let mut current = Table {
        store: own.clone(),
        points: before.clone(),
        next: next(&bytes)?,
    };
    let result = action(&mut current.points, &mut current.next)?;
    // A sole shared mount becomes private on MS_SLAVE; a shared/slave mount
    // keeps its prior upstream master when its final peer disappears.
    let conversions: Vec<_> = before
        .iter()
        .filter(|old| old.meta.peer != 0)
        .filter_map(|old| {
            current
                .points
                .iter()
                .find(|p| p.id == old.id && p.meta.peer == 0 && p.meta.master == old.meta.peer)
                .map(|_| (old.id, old.meta.peer, old.meta.master))
        })
        .collect();
    if !conversions.is_empty() {
        let mut peers: HashSet<_> = current
            .points
            .iter()
            .filter_map(|p| (p.meta.peer != 0).then_some(p.meta.peer))
            .collect();
        for store in shared::live_namespaces()? {
            if store.id() != own.id() {
                peers.extend(
                    decode_table(&store.read()?.1)?
                        .iter()
                        .filter_map(|p| (p.meta.peer != 0).then_some(p.meta.peer)),
                );
            }
        }
        for (id, group, upstream) in conversions {
            if !peers.contains(&group) {
                let point = current.points.iter_mut().find(|p| p.id == id).ok_or(EIO)?;
                point.meta.master = upstream;
                point.flags = (point.flags & !MS_PROPAGATION)
                    | if upstream == 0 { MS_PRIVATE } else { MS_SLAVE };
            }
        }
    }

    let old: HashMap<_, _> = before.iter().map(|p| (p.id, p)).collect();
    let new: HashMap<_, _> = current.points.iter().map(|p| (p.id, p)).collect();
    let removed: HashSet<_> = before
        .iter()
        .filter(|p| {
            new.get(&p.id)
                .is_none_or(|q| q.target != p.target || q.parent != p.parent)
        })
        .map(|p| p.id)
        .collect();
    let added: HashSet<_> = current
        .points
        .iter()
        .filter(|p| {
            old.get(&p.id)
                .is_none_or(|q| q.target != p.target || q.parent != p.parent)
        })
        .map(|p| p.id)
        .collect();
    let removal_roots: Vec<_> = before
        .iter()
        .filter(|p| removed.contains(&p.id) && !removed.contains(&p.parent))
        .cloned()
        .collect();
    let addition_roots: Vec<_> = current
        .points
        .iter()
        .filter(|p| added.contains(&p.id) && !added.contains(&p.parent))
        .map(|p| p.id)
        .collect();
    let needed = removal_roots
        .iter()
        .any(|p| old.get(&p.parent).is_some_and(|p| p.meta.peer != 0))
        || addition_roots.iter().any(|id| {
            new.get(id)
                .and_then(|p| new.get(&p.parent))
                .is_some_and(|p| p.meta.peer != 0)
        });
    drop(new);
    if !needed {
        let bytes = encode_table_with_next_id(&current.points, current.next)?;
        own.update(|_| Ok((bytes, ())))?;
        return Ok(result);
    }
    let mut tables = vec![current];
    for store in shared::live_namespaces()? {
        if store.id() == own.id() {
            continue;
        }
        let (_, bytes) = store.read()?;
        tables.push(Table {
            points: decode_table(&bytes)?,
            next: next(&bytes)?,
            store,
        });
    }
    for root in removal_roots {
        let Some(parent) = old.get(&root.parent).filter(|p| p.meta.peer != 0) else {
            continue;
        };
        let rest = suffix(&root.target, &parent.target).ok_or(EIO)?;
        for (table, destination, _) in destinations(&tables, parent.meta.peer) {
            if table == 0 && destination.id == parent.id {
                continue;
            }
            let target = join(&destination.target, rest);
            let selected = tables[table]
                .points
                .iter()
                .find(|p| p.parent == destination.id && p.target == target)
                .map(|p| p.id);
            if let Some(selected) = selected {
                let ids = api::descendants(&tables[table].points, selected);
                tables[table].points.retain(|p| !ids.contains(&p.id));
            }
        }
    }
    // Select receivers before publishing any part of this mount operation.
    // A new bind can inherit its source's peer group, but must not receive its
    // own creation event (nor another event from this same attachment tree).
    let mut receivers = HashMap::new();
    for root_id in &addition_roots {
        let group = tables[0]
            .points
            .iter()
            .find(|p| p.id == *root_id)
            .and_then(|root| tables[0].points.iter().find(|p| p.id == root.parent))
            .map(|parent| parent.meta.peer)
            .filter(|group| *group != 0);
        if let Some(group) = group {
            receivers.entry(group).or_insert_with(|| {
                destinations(&tables, group)
                    .into_iter()
                    .filter(|(index, point, _)| *index != 0 || !added.contains(&point.id))
                    .collect::<Vec<_>>()
            });
        }
    }
    for root_id in addition_roots {
        let root = tables[0]
            .points
            .iter()
            .find(|p| p.id == root_id)
            .cloned()
            .ok_or(EIO)?;
        let Some(parent) = tables[0]
            .points
            .iter()
            .find(|p| p.id == root.parent && p.meta.peer != 0)
            .cloned()
        else {
            continue;
        };
        let ids = api::descendants(&tables[0].points, root_id);
        for point in tables[0].points.iter_mut().filter(|p| ids.contains(&p.id)) {
            if point.meta.peer == 0 {
                point.meta.peer = shared::next_group()?;
            }
            point.flags = (point.flags & !MS_PRIVATE) | MS_SHARED;
        }
        let subtree: Vec<_> = tables[0]
            .points
            .iter()
            .filter(|p| ids.contains(&p.id))
            .cloned()
            .collect();
        let targets = receivers.get(&parent.meta.peer).ok_or(EIO)?;
        // Allocate child peer groups before connecting their masters, so a
        // grandchild slave follows the intermediate shared/slave group rather
        // than bypassing it and attaching directly to the original source.
        let mut slave_groups = HashMap::new();
        for source in &subtree {
            slave_groups.insert((source.meta.peer, parent.meta.peer), source.meta.peer);
            for (_, destination, slave) in targets {
                if *slave && destination.meta.peer != 0 {
                    let key = (source.meta.peer, destination.meta.peer);
                    if let std::collections::hash_map::Entry::Vacant(slot) = slave_groups.entry(key)
                    {
                        slot.insert(shared::next_group()?);
                    }
                }
            }
        }
        for (index, destination, slave) in targets {
            if *index == 0 && destination.id == parent.id {
                continue;
            }
            let table = &mut tables[*index];
            let mut translated = HashMap::from([(root.parent, destination.id)]);
            for source in &subtree {
                let mut point = source.clone();
                point.id = allocate_id(&mut table.next)?;
                translated.insert(source.id, point.id);
                point.parent = *translated.get(&source.parent).ok_or(EIO)?;
                point.target = join(
                    &destination.target,
                    suffix(&source.target, &parent.target).ok_or(EIO)?,
                );
                if *slave {
                    point.meta.master = *slave_groups
                        .get(&(source.meta.peer, destination.meta.master))
                        .ok_or(EIO)?;
                    point.meta.peer = if destination.meta.peer == 0 {
                        0
                    } else {
                        *slave_groups
                            .get(&(source.meta.peer, destination.meta.peer))
                            .ok_or(EIO)?
                    };
                    point.flags = (point.flags & !MS_PROPAGATION)
                        | MS_SLAVE
                        | if point.meta.peer != 0 { MS_SHARED } else { 0 };
                }
                // A mount propagated onto an already mounted location covers
                // that attachment, just like a local mount event.
                if let Some(existing) = table
                    .points
                    .iter()
                    .find(|p| p.parent == point.parent && p.target == point.target)
                {
                    point.parent = existing.id;
                }
                table.points.push(point);
            }
        }
    }
    let encoded = tables
        .iter()
        .map(|t| encode_table_with_next_id(&t.points, t.next))
        .collect::<Result<Vec<_>, _>>()?;
    for (table, bytes) in tables.iter().zip(&encoded) {
        table.store.reserve_update(bytes.len())?;
    }
    for (table, bytes) in tables.into_iter().zip(encoded) {
        table.store.update(|_| Ok((bytes, ())))?;
    }
    Ok(result)
}
fn destinations(tables: &[Table], group: u64) -> Vec<(usize, MountPoint, bool)> {
    let mut groups = HashSet::from([group]);
    loop {
        let before = groups.len();
        for point in tables.iter().flat_map(|t| &t.points) {
            if point.meta.master != 0 && groups.contains(&point.meta.master) && point.meta.peer != 0
            {
                groups.insert(point.meta.peer);
            }
        }
        if groups.len() == before {
            break;
        }
    }
    let mut result = Vec::new();
    for (i, table) in tables.iter().enumerate() {
        for p in &table.points {
            if p.meta.peer == group {
                result.push((i, p.clone(), false));
            } else if p.meta.master != 0 && groups.contains(&p.meta.master) {
                result.push((i, p.clone(), true));
            }
        }
    }
    result
}
