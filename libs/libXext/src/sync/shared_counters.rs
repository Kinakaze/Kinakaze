//! Session-owned SYNC counters; alarms stay on the Display that created them.
use kinakaze_libX11::shared;

pub fn create(display: usize, value: i64) -> Option<usize> {
    shared::transaction(|tx| {
        let id = tx
            .get("sync/next")
            .map_or(0x2000_0000, |v| u32::from_le_bytes(v.try_into().unwrap()));
        if id == u32::MAX {
            return Err(11);
        }
        tx.set("sync/next".into(), (id + 1).to_le_bytes().to_vec());
        let mut record = shared::pid().to_le_bytes().to_vec();
        record.extend_from_slice(&(display as u64).to_le_bytes());
        record.extend_from_slice(&value.to_le_bytes());
        tx.set(format!("sync/counter/{id}"), record);
        Ok(id as usize)
    })
    .ok()
}
pub fn value(id: usize) -> Option<i64> {
    let record = shared::get(&format!("sync/counter/{id}"))?;
    (record.len() == 20).then(|| i64::from_le_bytes(record[12..20].try_into().unwrap()))
}
fn observers(tx: &shared::Transaction<'_>, id: usize) -> std::collections::BTreeSet<u32> {
    let prefix = format!("sync/watch/{id}/");
    tx.prefix(&prefix)
        .into_iter()
        .filter_map(|(key, _)| key[prefix.len()..].split('/').next()?.parse().ok())
        .collect()
}
fn wake(owners: std::collections::BTreeSet<u32>) {
    let mut wire = [0u8; 32];
    wire[0] = 254; // Wake extension poll hooks; this is not an X wire event.
    for owner in owners {
        if owner != shared::pid() {
            shared::send(owner, wire);
        }
    }
}
pub fn set(id: usize, amount: i64, add: bool) -> Result<i64, u8> {
    let (value, owners) = shared::transaction(|tx| {
        let key = format!("sync/counter/{id}");
        let mut record = tx
            .get(&key)
            .filter(|v| v.len() == 20)
            .ok_or(152u8)?
            .to_vec();
        let old = i64::from_le_bytes(record[12..20].try_into().unwrap());
        let value = if add {
            old.checked_add(amount).ok_or(2u8)?
        } else {
            amount
        };
        record[12..20].copy_from_slice(&value.to_le_bytes());
        tx.set(key, record);
        Ok((value, observers(tx, id)))
    })?;
    kinakaze_libX11::frame_sync::trace(format_args!(
        "counter={id:#x} value={value} observers={owners:?}"
    ));
    wake(owners);
    Ok(value)
}
pub fn destroy(id: usize, owner_only: bool) -> bool {
    let result = shared::transaction(|tx| {
        let key = format!("sync/counter/{id}");
        let record = tx.get(&key).ok_or(152u8)?;
        if owner_only && record[..4] != shared::pid().to_le_bytes() {
            return Err(152);
        }
        tx.remove(&key);
        Ok(observers(tx, id))
    });
    if let Ok(owners) = result {
        wake(owners);
        true
    } else {
        false
    }
}
pub fn watch(counter: usize, alarm: usize) {
    if counter >= 0x2000_0000 {
        let _ = shared::set(
            format!("sync/watch/{counter}/{}/{alarm}", shared::pid()),
            Vec::new(),
        );
    }
}
pub fn unwatch(counter: usize, alarm: usize) {
    shared::remove(&format!("sync/watch/{counter}/{}/{alarm}", shared::pid()));
}
