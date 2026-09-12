//! nf_tables rules stored in their owning network namespace.
//!
//! Routes, sysctls and firewall rules share one crash-safe publication and
//! lifetime. No root-keyed files or separate root-only storage implementation.
use crate::{EINVAL, ENOSPC};
const EFBIG: i32 = 27;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attribute {
    pub kind: u16,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Object {
    pub message: u16,
    pub family: u8,
    pub handle: u64,
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Ruleset {
    pub generation: u32,
    pub next_handle: u64,
    pub objects: Vec<Object>,
}

impl Default for Ruleset {
    fn default() -> Self {
        Self {
            generation: 0,
            next_handle: 1,
            objects: Vec::new(),
        }
    }
}

impl Ruleset {
    pub fn allocate_handle(&mut self) -> Result<u64, i32> {
        let handle = self.next_handle.max(1);
        self.next_handle = handle.checked_add(1).ok_or(ENOSPC)?;
        Ok(handle)
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], i32> {
    let end = cursor.checked_add(N).ok_or(EINVAL)?;
    let value = bytes.get(*cursor..end).ok_or(EINVAL)?;
    *cursor = end;
    value.try_into().map_err(|_| EINVAL)
}

fn serialize(state: &Ruleset) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    put_u32(
        &mut out,
        u32::try_from(state.objects.len()).map_err(|_| EFBIG)?,
    );
    for object in &state.objects {
        put_u16(&mut out, object.message);
        out.push(object.family);
        out.push(0);
        put_u64(&mut out, object.handle);
        put_u32(
            &mut out,
            u32::try_from(object.attributes.len()).map_err(|_| EFBIG)?,
        );
        for attribute in &object.attributes {
            put_u16(&mut out, attribute.kind);
            put_u16(&mut out, 0);
            put_u32(
                &mut out,
                u32::try_from(attribute.value.len()).map_err(|_| EFBIG)?,
            );
            out.extend_from_slice(&attribute.value);
        }
    }
    Ok(out)
}

fn deserialize(generation: u32, next_handle: u64, bytes: &[u8]) -> Result<Ruleset, i32> {
    let mut cursor = 0usize;
    let count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        let message = u16::from_le_bytes(take(bytes, &mut cursor)?);
        let family = take::<1>(bytes, &mut cursor)?[0];
        let _reserved = take::<1>(bytes, &mut cursor)?;
        let handle = u64::from_le_bytes(take(bytes, &mut cursor)?);
        let attribute_count = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
        let mut attributes = Vec::with_capacity(attribute_count);
        for _ in 0..attribute_count {
            let kind = u16::from_le_bytes(take(bytes, &mut cursor)?);
            let _reserved = take::<2>(bytes, &mut cursor)?;
            let length = u32::from_le_bytes(take(bytes, &mut cursor)?) as usize;
            let end = cursor.checked_add(length).ok_or(EINVAL)?;
            let value = bytes.get(cursor..end).ok_or(EINVAL)?.to_vec();
            cursor = end;
            attributes.push(Attribute { kind, value });
        }
        objects.push(Object {
            message,
            family,
            handle,
            attributes,
        });
    }
    if cursor != bytes.len() {
        return Err(EINVAL);
    }
    Ok(Ruleset {
        generation,
        next_handle: next_handle.max(1),
        objects,
    })
}

fn decode_namespace(bytes: &[u8]) -> Result<Ruleset, i32> {
    if bytes.is_empty() {
        return Ok(Ruleset::default());
    }
    if bytes.len() < 12 {
        return Err(EINVAL);
    }
    deserialize(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        &bytes[12..],
    )
}

pub(crate) fn snapshot() -> Result<Ruleset, i32> {
    decode_namespace(&crate::route_state::snapshot()?.netfilter)
}

pub(crate) fn transaction<T, E>(
    expected_generation: u32,
    body: impl FnOnce(&mut Ruleset) -> Result<T, E>,
) -> Result<Result<T, E>, i32> {
    enum Failure<E> {
        Storage(i32),
        Mutation(E),
    }
    let outcome = crate::route_state::transaction(|network| {
        let mut state = decode_namespace(&network.netfilter).map_err(Failure::Storage)?;
        if state.generation != expected_generation {
            return Err(Failure::Storage(85)); // ERESTART: nfnetlink generation validation.
        }
        let value = body(&mut state).map_err(Failure::Mutation)?;
        state.generation = state.generation.wrapping_add(1);
        let mut bytes = state.generation.to_le_bytes().to_vec();
        bytes.extend_from_slice(&state.next_handle.to_le_bytes());
        bytes.extend_from_slice(&serialize(&state).map_err(Failure::Storage)?);
        network.netfilter = bytes;
        Ok(value)
    })?;
    match outcome {
        Ok(value) => Ok(Ok(value)),
        Err(Failure::Storage(error)) => Err(error),
        Err(Failure::Mutation(error)) => Ok(Err(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_ruleset_uses_namespace_storage_and_preserves_failed_transactions() {
        let before = crate::route_state::snapshot().unwrap();
        let initial = snapshot().unwrap();
        transaction(initial.generation, |state| {
            let handle = state.allocate_handle()?;
            state.objects.push(Object {
                message: 6,
                family: 2,
                handle,
                attributes: Vec::new(),
            });
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        let committed = snapshot().unwrap();
        let committed_network = crate::route_state::snapshot().unwrap();
        assert_eq!(committed.generation, initial.generation.wrapping_add(1));
        assert_eq!(
            decode_namespace(&crate::route_state::snapshot().unwrap().netfilter).unwrap(),
            committed
        );
        assert_eq!(
            transaction(initial.generation, |_| -> Result<(), i32> {
                panic!("stale generation must not execute its mutation")
            }),
            Err(85)
        );
        assert_eq!(
            transaction(committed.generation, |state| {
                state.objects.clear();
                Err::<(), _>(22)
            }),
            Ok(Err(22))
        );
        assert_eq!(snapshot().unwrap(), committed);
        assert_eq!(crate::route_state::snapshot().unwrap(), committed_network);
        crate::route_state::transaction(|network| {
            *network = before;
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
    }

    #[test]
    fn object_encoding_round_trips_nested_attribute_bytes() {
        let state = Ruleset {
            generation: 19,
            next_handle: 44,
            objects: vec![Object {
                message: 6,
                family: 2,
                handle: 43,
                attributes: vec![Attribute {
                    kind: 4,
                    value: vec![0, 1, 2, 3, 0xff],
                }],
            }],
        };
        let bytes = serialize(&state).unwrap();
        assert_eq!(deserialize(19, 44, &bytes).unwrap(), state);
    }
}
