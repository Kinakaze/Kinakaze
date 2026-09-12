//! Fork records describe pinned images, never executable callback addresses.
//! The child resolves the declared management export from each restored module.
use kinakaze_v2_abi::{ProviderInitializeV1, RuntimeApiResolverV1, RuntimeApiV1, STATUS_OK};
use kinakaze_v2_host_win::LoadedModule;
use std::{
    mem,
    panic::{AssertUnwindSafe, catch_unwind},
    slice,
    sync::{Mutex, OnceLock},
};

const KEY: u64 = 0x5632_5052_4f56_4232; // V2PROVB2; image identity records, version 2.
const MAX_BINDINGS: usize = 128;
const HEADER: usize = 16;
const RECORD: usize = 16;
static BINDINGS: Mutex<Vec<Binding>> = Mutex::new(Vec::new());
static RESOLVE_API: OnceLock<RuntimeApiResolverV1> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub module_base: usize,
    pub image_len: usize,
}

/// Install before native fork bootstrap. The resolver supplies a fresh child
/// table after session adoption, without making this crate own a session.
pub fn install(resolver: RuntimeApiResolverV1) -> Result<(), i32> {
    let installed = RESOLVE_API.get_or_init(|| resolver);
    if !std::ptr::fn_addr_eq(*installed, resolver) {
        return Err(22);
    }
    guest_link::provider::install_registry_loader(restore_registry).map_err(|_| 22)?;
    if !guest_process::register_fork_participant(guest_process::ForkParticipant {
        abi: guest_process::FORK_PARTICIPANT_ABI,
        priority: -10_000,
        key: KEY,
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(restore),
    }) {
        return Err(5);
    }
    Ok(())
}

fn restore_registry(
    source: &std::path::Path,
) -> Result<std::sync::Arc<guest_link::ProviderRegistry>, guest_link::LinkError> {
    let resolver = RESOLVE_API.get().ok_or_else(|| {
        guest_link::LinkError::InvalidProvider("runtime API resolver unavailable".into())
    })?;
    let mut api = mem::MaybeUninit::<RuntimeApiV1>::uninit();
    if unsafe { resolver(api.as_mut_ptr()) } != STATUS_OK {
        return Err(guest_link::LinkError::InvalidProvider(
            "child runtime API unavailable".into(),
        ));
    }
    crate::providers::load(source, &unsafe { api.assume_init() })
        .map_err(|error| guest_link::LinkError::InvalidProvider(error.to_string()))
}

pub(super) fn insert(binding: Binding) -> Result<(), i32> {
    if !valid(binding) {
        return Err(22);
    }
    let mut bindings = BINDINGS.lock().map_err(|_| 5)?;
    if bindings.len() >= MAX_BINDINGS {
        return Err(12);
    }
    if bindings
        .iter()
        .any(|old| old.module_base == binding.module_base)
    {
        return Err(17);
    }
    bindings.push(binding);
    Ok(())
}

pub(super) fn remove(base: usize) {
    if let Ok(mut bindings) = BINDINGS.lock() {
        bindings.retain(|binding| binding.module_base != base);
    }
}

fn valid(binding: Binding) -> bool {
    binding.module_base != 0
        && binding.image_len != 0
        && binding.module_base.checked_add(binding.image_len).is_some()
}

fn encode(bindings: &[Binding], output: &mut [u8]) -> Result<usize, i32> {
    let length = HEADER + bindings.len() * RECORD;
    if bindings.len() > MAX_BINDINGS || output.len() < length {
        return Err(22);
    }
    output[..8].copy_from_slice(&KEY.to_le_bytes());
    output[8..HEADER].copy_from_slice(&(bindings.len() as u64).to_le_bytes());
    for (binding, record) in bindings
        .iter()
        .zip(output[HEADER..length].chunks_exact_mut(RECORD))
    {
        record[..8].copy_from_slice(&(binding.module_base as u64).to_le_bytes());
        record[8..].copy_from_slice(&(binding.image_len as u64).to_le_bytes());
    }
    Ok(length)
}

fn decode(bytes: &[u8]) -> Result<Vec<Binding>, i32> {
    let word = |start| {
        bytes
            .get(start..start + 8)
            .and_then(|b| <[u8; 8]>::try_from(b).ok())
            .map(u64::from_le_bytes)
            .ok_or(22)
    };
    if word(0)? != KEY {
        return Err(22);
    }
    let count = usize::try_from(word(8)?).map_err(|_| 22)?;
    if count > MAX_BINDINGS || bytes.len() != HEADER + count * RECORD {
        return Err(22);
    }
    let mut bindings: Vec<Binding> = Vec::with_capacity(count);
    for index in 0..count {
        let start = HEADER + index * RECORD;
        let binding = Binding {
            module_base: usize::try_from(word(start)?).map_err(|_| 22)?,
            image_len: usize::try_from(word(start + 8)?).map_err(|_| 22)?,
        };
        if !valid(binding)
            || bindings
                .iter()
                .any(|old| old.module_base == binding.module_base)
        {
            return Err(22);
        }
        bindings.push(binding);
    }
    Ok(bindings)
}

unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
    match catch_unwind(AssertUnwindSafe(|| -> Result<usize, i32> {
        let bindings = BINDINGS.lock().map_err(|_| 5)?;
        let length = HEADER + bindings.len() * RECORD;
        if buffer.is_null() {
            return Ok(length);
        }
        if capacity < length {
            return Err(22);
        }
        // SAFETY: coordinator owns this writable span; write directly without
        // allocating another complete payload during each fork size/copy pass.
        encode(&bindings, unsafe {
            slice::from_raw_parts_mut(buffer, length)
        })
    })) {
        Ok(Ok(length)) => length as isize,
        Ok(Err(error)) => -(error as isize),
        Err(_) => -5,
    }
}

fn restore_bindings(bindings: &[Binding], api: &RuntimeApiV1) -> Result<(), i32> {
    for binding in bindings {
        // The coordinator already verified/reloaded the image at its base.
        // Pin it again while resolving/calling; never trust a serialized PC.
        let module = LoadedModule::pin(binding.module_base).map_err(|_| 22)?;
        if module.mapped_len() != binding.image_len {
            return Err(22);
        }
        let address =
            unsafe { module.symbol(c"kinakaze_provider_initialize_v1") }.map_err(|_| 22)?;
        if !(binding.module_base..binding.module_base + binding.image_len)
            .contains(&(address as usize))
        {
            return Err(22);
        }
        // SAFETY: the module implements this fixed native ABI; the owned image was
        // restored and the export is within its own validated image.
        let initialize: ProviderInitializeV1 = unsafe { mem::transmute(address) };
        if unsafe { initialize(api) } != STATUS_OK {
            return Err(5);
        }
    }
    Ok(())
}

unsafe extern "system" fn restore(payload: *const u8, length: usize) -> i32 {
    match catch_unwind(AssertUnwindSafe(|| -> Result<(), i32> {
        if payload.is_null() || length > HEADER + MAX_BINDINGS * RECORD {
            return Err(22);
        }
        let bindings = decode(unsafe { slice::from_raw_parts(payload, length) })?;
        let resolver = RESOLVE_API.get().ok_or(5)?;
        let mut api = mem::MaybeUninit::<RuntimeApiV1>::uninit();
        if unsafe { resolver(api.as_mut_ptr()) } != STATUS_OK {
            return Err(5);
        }
        let api = unsafe { api.assume_init() };
        restore_bindings(&bindings, &api)?;
        *BINDINGS.lock().map_err(|_| 5)? = bindings;
        Ok(())
    })) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => error,
        Err(_) => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_rejects_truncation_duplicate_images_and_overflow() {
        let binding = Binding {
            module_base: 0x10000,
            image_len: 0x2000,
        };
        let mut bytes = vec![0; HEADER + RECORD];
        assert_eq!(encode(&[binding], &mut bytes), Ok(bytes.len()));
        assert_eq!(decode(&bytes).unwrap(), [binding]);
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        assert!(encode(&[binding], &mut bytes[..HEADER]).is_err());
        let mut duplicate = vec![0; HEADER + 2 * RECORD];
        encode(&[binding, binding], &mut duplicate).unwrap();
        assert!(decode(&duplicate).is_err());
        bytes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode(&bytes).is_err());
        let mut bytes = vec![0; HEADER + RECORD];
        encode(
            &[Binding {
                module_base: usize::MAX,
                image_len: 1,
            }],
            &mut bytes,
        )
        .unwrap();
        assert!(decode(&bytes).is_err());
        encode(
            &[Binding {
                module_base: 0x10000,
                image_len: 0,
            }],
            &mut bytes,
        )
        .unwrap();
        assert!(decode(&bytes).is_err());
    }
}
