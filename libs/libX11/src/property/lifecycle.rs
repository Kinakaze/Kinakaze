//! Fork transfers names and property bytes, never native containers or locks.
use super::{Property, STORE, Store, atoms};
use core::{cell::RefCell, ptr};
use std::sync::MutexGuard;
const MAGIC: u64 = u64::from_le_bytes(*b"CYXPROP4");
const FOREIGN: usize = 1 << 63;
struct Frozen {
    _store: MutexGuard<'static, Store>,
    bytes: Vec<u8>,
}
thread_local! { static FROZEN: RefCell<Option<Frozen>> = const { RefCell::new(None) }; }
fn word(bytes: &mut Vec<u8>, value: usize) {
    bytes.extend_from_slice(&(value as u64).to_le_bytes());
}
fn blob(bytes: &mut Vec<u8>, value: &[u8]) {
    word(bytes, value.len());
    bytes.extend_from_slice(value);
}
fn encode(store: &Store) -> Result<Vec<u8>, i32> {
    let mut bytes = Vec::new();
    word(&mut bytes, MAGIC as usize);
    word(&mut bytes, store.atoms.len());
    for name in &store.atoms {
        blob(&mut bytes, name);
    }
    word(&mut bytes, store.properties.len());
    for ((window, atom), value) in &store.properties {
        let logical = if *window == 1 {
            0
        } else {
            kinakaze_libdisplay::window::logical_for_native(*window).ok_or(95)? as usize
        };
        for value in [logical, *atom, value.kind, value.format as usize] {
            word(&mut bytes, value);
        }
        blob(&mut bytes, &value.bytes);
    }
    word(&mut bytes, store.selections.len());
    for (atom, owner) in &store.selections {
        let logical = match owner.window {
            0 => usize::MAX,
            1 => 0,
            w => kinakaze_libdisplay::window::logical_for_native(w).ok_or(95)? as usize,
        };
        for v in [*atom, logical, owner.time as usize] {
            word(&mut bytes, v);
        }
    }
    word(&mut bytes, store.masks.len());
    for (window, mask) in &store.masks {
        let logical = if *window == 1 {
            0
        } else {
            kinakaze_libdisplay::window::logical_for_native(*window)
                .map_or(*window | FOREIGN, |id| id as usize)
        };
        word(&mut bytes, logical);
        word(&mut bytes, *mask as usize);
    }
    Ok(bytes)
}
struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, size: usize) -> Result<&[u8], i32> {
        let (value, tail) = self.0.split_at_checked(size).ok_or(22)?;
        self.0 = tail;
        Ok(value)
    }
    fn word(&mut self) -> Result<usize, i32> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()) as usize)
    }
    fn count(&mut self) -> Result<usize, i32> {
        let value = self.word()?;
        if value > self.0.len() / 8 {
            Err(22)
        } else {
            Ok(value)
        }
    }
    fn blob(&mut self) -> Result<Vec<u8>, i32> {
        let size = self.word()?;
        Ok(self.take(size)?.to_vec())
    }
}
fn decode(bytes: &[u8]) -> Result<Store, i32> {
    let mut input = Reader(bytes);
    if input.word()? != MAGIC as usize {
        return Err(22);
    }
    let mut store = Store {
        atoms: Vec::new(),
        properties: Default::default(),
        selections: Default::default(),
        masks: Default::default(),
    };
    let mut unique = std::collections::BTreeSet::new();
    for _ in 0..input.count()? {
        let value = input.blob()?;
        if value.contains(&0) || !unique.insert(value.clone()) {
            return Err(22);
        }
        store.atoms.push(value);
    }
    for _ in 0..input.count()? {
        let logical = input.word()?;
        let window = if logical == 0 {
            1
        } else {
            kinakaze_libdisplay::window::native_handle(logical as u64).ok_or(22)?
        };
        let atom = input.word()?;
        let kind = input.word()?;
        let format = input.word()?;
        let bytes = input.blob()?;
        if !matches!(format, 8 | 16 | 32)
            || !atoms::valid(&store, atom)
            || !atoms::valid(&store, kind)
            || bytes.len() % (format / 8) != 0
        {
            return Err(22);
        }
        if store
            .properties
            .insert(
                (window, atom),
                Property {
                    kind,
                    format: format as i32,
                    bytes,
                },
            )
            .is_some()
        {
            return Err(22);
        }
    }
    for _ in 0..input.count()? {
        let atom = input.word()?;
        let logical = input.word()?;
        let time = u32::try_from(input.word()?).map_err(|_| 22)?;
        let window = match logical {
            usize::MAX => 0,
            0 => 1,
            id => kinakaze_libdisplay::window::native_handle(id as u64).ok_or(22)?,
        };
        if !atoms::valid(&store, atom)
            || store
                .selections
                .insert(atom, super::selection::Owner { window, time })
                .is_some()
        {
            return Err(22);
        }
    }
    for _ in 0..input.count()? {
        let logical = input.word()?;
        let window = if logical == 0 {
            1
        } else if logical & FOREIGN != 0 {
            // Foreign subscriptions still name the original server window.
            logical & !FOREIGN
        } else {
            kinakaze_libdisplay::window::native_handle(logical as u64).ok_or(22)?
        };
        let mask = u32::try_from(input.word()?).map_err(|_| 22)?;
        if mask & !0x01ff_ffff != 0 || store.masks.insert(window, mask).is_some() {
            return Err(22);
        }
    }
    if !input.0.is_empty() {
        return Err(22);
    }
    Ok(store)
}
unsafe extern "system" fn prepare() -> i32 {
    FROZEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return 35;
        }
        let Ok(mut store) = STORE.try_lock() else {
            return 11;
        };
        // Shared properties belong to native windows; fork rebuilds those
        // handles. Snapshot only this client's windows, never the shared root.
        let rows = match crate::shared::transaction(|tx| Ok(tx.prefix("p/"))) {
            Ok(rows) => rows,
            Err(_) => return 11,
        };
        store.properties.clear();
        for (key, bytes) in rows {
            let mut fields = key.split('/').skip(1);
            let (Some(window), Some(atom)) = (
                fields.next().and_then(|v| v.parse::<usize>().ok()),
                fields.next().and_then(|v| v.parse::<usize>().ok()),
            ) else {
                continue;
            };
            if window != 1
                && kinakaze_libdisplay::window::logical_for_native(window).is_some()
                && let Some(value) = super::decode_property(&bytes)
            {
                store.properties.insert((window, atom), value);
            }
        }
        let bytes = match encode(&store) {
            Ok(bytes) => bytes,
            Err(error) => return error,
        };
        *slot = Some(Frozen {
            _store: store,
            bytes,
        });
        0
    })
}
unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    FROZEN.with(|slot| {
        let slot = slot.borrow();
        let Some(frozen) = slot.as_ref() else {
            return -22;
        };
        if !output.is_null() {
            if capacity < frozen.bytes.len() {
                return -22;
            }
            unsafe {
                ptr::copy_nonoverlapping(frozen.bytes.as_ptr(), output, frozen.bytes.len());
            }
        }
        frozen.bytes.len() as isize
    })
}
unsafe extern "system" fn parent(_status: i32) {
    FROZEN.with(|slot| {
        slot.borrow_mut().take();
    });
}
unsafe extern "system" fn child(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length > isize::MAX as usize {
        return 22;
    }
    match decode(unsafe { core::slice::from_raw_parts(input, length) }) {
        Ok(store) => {
            let mut parents = vec![1usize];
            while let Some(parent) = parents.pop() {
                if let Some((_, children)) =
                    kinakaze_libdisplay::window::hierarchy((parent != 1).then_some(parent))
                {
                    for window in children {
                        crate::shared::register_window(window, parent);
                        parents.push(window);
                    }
                }
            }
            for (&(window, atom), value) in &store.properties {
                if let Err(code) = super::update_property(window, atom, |slot| {
                    *slot = Some(value.clone());
                    Ok(())
                }) {
                    return code as i32;
                }
            }
            for (&window, &mask) in &store.masks {
                let _ = crate::shared::subscribe(window, mask);
            }
            *STORE.lock().unwrap() = store;
            super::decorations::restore();
            0
        }
        Err(error) => error,
    }
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 449,
        key: MAGIC,
        prepare: Some(prepare),
        snapshot: Some(snapshot),
        parent: Some(parent),
        child: Some(child),
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_validates_boundaries_and_property_width() {
        let mut store = Store {
            atoms: vec![b"probe".to_vec()],
            properties: Default::default(),
            selections: Default::default(),
            masks: Default::default(),
        };
        store.properties.insert(
            (1, 69),
            Property {
                kind: 31,
                format: 8,
                bytes: b"abc".to_vec(),
            },
        );
        let bytes = encode(&store).unwrap();
        assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err());
        }
        store.properties.get_mut(&(1, 69)).unwrap().format = 32;
        assert!(decode(&encode(&store).unwrap()).is_err());
    }
}
