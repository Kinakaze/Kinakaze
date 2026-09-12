//! AOT instruction patch cache for ELF executables and shared objects.
//!
//! When an ELF object is first loaded, instruction decoding and scanning for
//! raw `syscall` sites is performed and saved
//! into `<rootfs>/var/cache/kinakaze/aot/<hash>.aot`. Subsequent launches
//! validate the cached patch sites against the mapped image before applying
//! them, avoiding repeated instruction decoding.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use super::ExecutionError;
use super::Image;

// Version 26 also follows a checked switch index copied to another register.
// Old plans can otherwise leave reachable GS accesses untranslated.
const CACHE_MAGIC: &[u8; 8] = b"CRYAOT26";
const ARCH_X86_64: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedCanarySite {
    pub offset: u32,
    pub len: u8,
    pub prefix_offset: u8,
    pub displacement_offset: u8,
    pub displacement_size: u8,
    pub original_displacement: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedSyscallSite {
    pub offset: u32,
    pub len: u8,
    /// Whether control-flow analysis proved a five-byte detour cannot cover an
    /// independent entry point. False sites retain a two-byte UD2 trap.
    pub trampoline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedThreadPointerSite {
    pub offset: u32,
    /// Low seven bits: instruction length. Bit 7: virtual Linux GS instruction.
    pub len: u8,
}

#[derive(Clone, Debug, Default)]
pub struct CachedSegment {
    pub virtual_address: u64,
    pub memory_size: u64,
    pub canary_sites: Vec<CachedCanarySite>,
    pub syscall_sites: Vec<CachedSyscallSite>,
    pub thread_pointer_sites: Vec<CachedThreadPointerSite>,
}

#[derive(Clone, Debug, Default)]
pub struct CachedObjectAot {
    pub segments: Vec<CachedSegment>,
}

/// Computes a cache key for the complete ELF image.
///
/// Hashing only the first and last pages lets an in-place package upgrade retain
/// an old key when all changed code happens to be in the middle of the file.  AOT
/// records describe instruction boundaries, so that is not merely a stale-speed
/// problem: it can turn data in the new image into code.  The bytes are already in
/// memory for ELF parsing, making one complete linear pass both cheap and exact
/// with respect to the image being linked.
pub fn compute_cache_key(name: &str, data: &[u8]) -> String {
    // SIMD-dispatched full-content hashing removes the serial multiply per byte
    // without weakening invalidation to file timestamps or sampled pages.
    let hash = blake3::hash(data);
    let sanitized_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{sanitized_name}_{}.aot", hash.to_hex())
}

/// Attempts to load cached AOT patch sites from `<cache_dir>/<cache_key>`.
pub fn load_aot_cache(cache_dir: &Path, cache_key: &str) -> Option<CachedObjectAot> {
    let data = fs::read(cache_dir.join(cache_key)).ok()?;
    let mut reader = data.as_slice();

    let magic = take::<8>(&mut reader)?;
    if &magic != CACHE_MAGIC {
        return None;
    }

    let arch_bytes = take::<4>(&mut reader)?;
    if u32::from_le_bytes(arch_bytes) != ARCH_X86_64 {
        return None;
    }

    let seg_count = u32::from_le_bytes(take::<4>(&mut reader)?) as usize;
    if seg_count > reader.len() / 28 {
        return None;
    }

    let mut segments = Vec::with_capacity(seg_count);
    for _ in 0..seg_count {
        let virtual_address = u64::from_le_bytes(take::<8>(&mut reader)?);
        let memory_size = u64::from_le_bytes(take::<8>(&mut reader)?);
        let canary_count = u32::from_le_bytes(take::<4>(&mut reader)?) as usize;
        let syscall_count = u32::from_le_bytes(take::<4>(&mut reader)?) as usize;
        let thread_pointer_count = u32::from_le_bytes(take::<4>(&mut reader)?) as usize;
        let records_length = canary_count
            .checked_mul(12)?
            .checked_add(syscall_count.checked_mul(6)?)?
            .checked_add(thread_pointer_count.checked_mul(5)?)?;
        if records_length > reader.len() {
            return None;
        }

        let mut canary_sites = Vec::with_capacity(canary_count);
        for _ in 0..canary_count {
            let record = take::<12>(&mut reader)?;
            canary_sites.push(CachedCanarySite {
                offset: u32::from_le_bytes([record[0], record[1], record[2], record[3]]),
                len: record[4],
                prefix_offset: record[5],
                displacement_offset: record[6],
                displacement_size: record[7],
                original_displacement: u32::from_le_bytes([
                    record[8], record[9], record[10], record[11],
                ]),
            });
        }

        let mut syscall_sites = Vec::with_capacity(syscall_count);
        for _ in 0..syscall_count {
            let record = take::<6>(&mut reader)?;
            syscall_sites.push(CachedSyscallSite {
                offset: u32::from_le_bytes([record[0], record[1], record[2], record[3]]),
                len: record[4],
                trampoline: record[5] != 0,
            });
        }

        let mut thread_pointer_sites = Vec::with_capacity(thread_pointer_count);
        for _ in 0..thread_pointer_count {
            let record = take::<5>(&mut reader)?;
            thread_pointer_sites.push(CachedThreadPointerSite {
                offset: u32::from_le_bytes([record[0], record[1], record[2], record[3]]),
                len: record[4],
            });
        }

        segments.push(CachedSegment {
            virtual_address,
            memory_size,
            canary_sites,
            syscall_sites,
            thread_pointer_sites,
        });
    }

    Some(CachedObjectAot { segments })
}

fn take<const N: usize>(source: &mut &[u8]) -> Option<[u8; N]> {
    if source.len() < N {
        return None;
    }
    let (head, tail) = source.split_at(N);
    let value = head.try_into().ok()?;
    *source = tail;
    Some(value)
}

/// Saves collected AOT patch sites into `<cache_dir>/<cache_key>`.
pub fn save_aot_cache(cache_dir: &Path, cache_key: &str, aot: &CachedObjectAot) -> io::Result<()> {
    let _ = fs::create_dir_all(cache_dir);
    let target = cache_dir.join(cache_key);
    let temp = cache_dir.join(format!("{}.tmp.{}", cache_key, std::process::id()));

    let file = File::create(&temp)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(CACHE_MAGIC)?;
    writer.write_all(&ARCH_X86_64.to_le_bytes())?;
    writer.write_all(&(aot.segments.len() as u32).to_le_bytes())?;

    for seg in &aot.segments {
        writer.write_all(&seg.virtual_address.to_le_bytes())?;
        writer.write_all(&seg.memory_size.to_le_bytes())?;
        writer.write_all(&(seg.canary_sites.len() as u32).to_le_bytes())?;
        writer.write_all(&(seg.syscall_sites.len() as u32).to_le_bytes())?;
        writer.write_all(&(seg.thread_pointer_sites.len() as u32).to_le_bytes())?;

        for site in &seg.canary_sites {
            let mut record = [0u8; 12];
            record[0..4].copy_from_slice(&site.offset.to_le_bytes());
            record[4] = site.len;
            record[5] = site.prefix_offset;
            record[6] = site.displacement_offset;
            record[7] = site.displacement_size;
            record[8..12].copy_from_slice(&site.original_displacement.to_le_bytes());
            writer.write_all(&record)?;
        }

        for site in &seg.syscall_sites {
            let mut record = [0u8; 6];
            record[0..4].copy_from_slice(&site.offset.to_le_bytes());
            record[4] = site.len;
            record[5] = u8::from(site.trampoline);
            writer.write_all(&record)?;
        }

        for site in &seg.thread_pointer_sites {
            let mut record = [0u8; 5];
            record[0..4].copy_from_slice(&site.offset.to_le_bytes());
            record[4] = site.len;
            writer.write_all(&record)?;
        }
    }

    writer.flush()?;
    drop(writer);
    let _ = fs::rename(&temp, &target);
    Ok(())
}

/// Applies cached AOT patches directly to writable mapped segments in memory.
pub(super) fn validate_cached_aot(
    object: &Image<'_>,
    cached: &CachedObjectAot,
    canary_target: u32,
) -> Result<(), ExecutionError> {
    // Validate the complete cache before changing a single instruction.  The
    // caller can then discard an interrupted/corrupt/stale cache and perform a
    // fresh analysis against pristine mapped bytes.
    let elf = object.elf()?;
    let executable_segments = elf
        .program_headers()?
        .into_iter()
        .filter(|candidate| {
            candidate.kind == kinakaze_elf::PT_LOAD
                && candidate.flags & kinakaze_elf::PF_X != 0
                && candidate.memory_size != 0
        })
        .collect::<Vec<_>>();
    for segment in &cached.segments {
        if !executable_segments.iter().any(|candidate| {
            candidate.virtual_address == segment.virtual_address
                && candidate.memory_size == segment.memory_size
        }) {
            return Err(ExecutionError::AddressOverflow);
        }
        let segment_base = object.resolve_address(segment.virtual_address)?;
        let length =
            usize::try_from(segment.memory_size).map_err(|_| ExecutionError::AddressOverflow)?;
        let code = unsafe { core::slice::from_raw_parts(segment_base as *const u8, length) };
        for site in &segment.canary_sites {
            let offset = site.offset as usize;
            let end = offset
                .checked_add(site.len as usize)
                .ok_or(ExecutionError::AddressOverflow)?;
            let bytes = code
                .get(offset..end)
                .ok_or(ExecutionError::AddressOverflow)?;
            let mut decoder = iced_x86::Decoder::with_ip(
                64,
                bytes,
                (segment_base + offset) as u64,
                iced_x86::DecoderOptions::NONE,
            );
            let instruction = decoder.decode();
            let offsets =
                crate::execution::segment_patch::safe_get_constant_offsets(&decoder, &instruction)
                    .ok_or(ExecutionError::AddressOverflow)?;
            let planned = crate::execution::segment_patch::plan_patch(
                &instruction,
                offsets,
                bytes,
                canary_target,
                None,
            )
            .ok_or(ExecutionError::AddressOverflow)?;
            if instruction.is_invalid()
                || instruction.len() != site.len as usize
                || planned.prefix_offset != site.prefix_offset as usize
                || planned.displacement_offset != site.displacement_offset as usize
                || planned.displacement_size != site.displacement_size as usize
                || planned.original_displacement != u64::from(site.original_displacement)
            {
                return Err(ExecutionError::AddressOverflow);
            }
        }
        for site in &segment.syscall_sites {
            let offset = site.offset as usize;
            let bytes = code.get(offset..).ok_or(ExecutionError::AddressOverflow)?;
            let mut decoder = iced_x86::Decoder::with_ip(
                64,
                bytes,
                (segment_base + offset) as u64,
                iced_x86::DecoderOptions::NONE,
            );
            let instruction = decoder.decode();
            if instruction.is_invalid()
                || instruction.mnemonic() != iced_x86::Mnemonic::Syscall
                || instruction.len() != site.len as usize
            {
                return Err(ExecutionError::AddressOverflow);
            }
        }
        for site in &segment.thread_pointer_sites {
            let offset = site.offset as usize;
            if site.len & 0x80 != 0 {
                let bytes = code.get(offset..).ok_or(ExecutionError::AddressOverflow)?;
                let instruction = iced_x86::Decoder::with_ip(
                    64,
                    bytes,
                    (segment_base + offset) as u64,
                    iced_x86::DecoderOptions::NONE,
                )
                .decode();
                if instruction.is_invalid()
                    || !super::guest_gs::matches(&instruction)
                    || instruction.len() != (site.len & 0x7f) as usize
                {
                    return Err(ExecutionError::AddressOverflow);
                }
                continue;
            }
            if offset >= code.len()
                || unsafe {
                    crate::execution::instruction_trampoline::first_fs_instruction_len(
                        segment_base + offset,
                    )
                } != Some(site.len as usize)
            {
                return Err(ExecutionError::AddressOverflow);
            }
        }
    }

    Ok(())
}

pub(super) fn apply_cached_aot(
    object: &Image<'_>,
    cached: &CachedObjectAot,
    canary_target: u32,
    syscall_dispatcher: Option<usize>,
    thread_pointer_teb_offset: Option<usize>,
    host_transition_teb_offset: Option<usize>,
    scratch_teb_offset: Option<usize>,
) -> Result<usize, ExecutionError> {
    let mut patched_count = 0;

    for segment in &cached.segments {
        let Ok(segment_base) = object.resolve_address(segment.virtual_address) else {
            continue;
        };
        let length = segment.memory_size as usize;
        // SAFETY: mapped object memory is writable at this point in the link before protect().
        let code = unsafe { core::slice::from_raw_parts_mut(segment_base as *mut u8, length) };
        let unpublished =
            unsafe { super::instruction_trampoline::UnpublishedCode::new(segment_base, length) };

        // Canary rewrites must precede syscall detours. A syscall source window
        // may relocate a following canary instruction into its own trampoline.
        for site in &segment.canary_sites {
            let offset = site.offset as usize;
            let end = offset
                .checked_add(site.len as usize)
                .ok_or(ExecutionError::AddressOverflow)?;
            let bytes = code
                .get_mut(offset..end)
                .ok_or(ExecutionError::AddressOverflow)?;
            crate::execution::segment_patch::apply_patch(
                bytes,
                crate::execution::segment_patch::InstructionPatch {
                    prefix_offset: site.prefix_offset as usize,
                    displacement_offset: site.displacement_offset as usize,
                    displacement_size: site.displacement_size as usize,
                    original_displacement: u64::from(site.original_displacement),
                    new_displacement: u64::from(canary_target),
                },
            );
            patched_count += 1;
        }

        for site in &segment.syscall_sites {
            let offset = site.offset as usize;
            let address = segment_base
                .checked_add(offset)
                .ok_or(ExecutionError::AddressOverflow)?;
            if site.trampoline
                && let Some(dispatcher) = syscall_dispatcher
            {
                unsafe {
                    crate::execution::instruction_trampoline::install_syscall_unpublished(
                        address,
                        dispatcher,
                        thread_pointer_teb_offset,
                        host_transition_teb_offset,
                        &unpublished,
                    )
                }?;
            } else {
                let end = offset
                    .checked_add(site.len as usize)
                    .ok_or(ExecutionError::AddressOverflow)?;
                if end > code.len() || site.len < 2 {
                    return Err(ExecutionError::AddressOverflow);
                }
                code[offset..end].fill(0x90);
                code[offset..offset + 2].copy_from_slice(&[0x0f, 0x0b]);
            }
        }

        if let (Some(thread_pointer_teb_offset), Some(scratch_teb_offset)) =
            (thread_pointer_teb_offset, scratch_teb_offset)
        {
            for site in &segment.thread_pointer_sites {
                let address = segment_base + site.offset as usize;
                if site.len & 0x80 != 0 {
                    unsafe {
                        super::guest_gs::install(
                            address,
                            host_transition_teb_offset.ok_or(ExecutionError::AddressOverflow)?,
                        )
                    }?;
                    continue;
                }
                unsafe {
                    crate::execution::instruction_trampoline::install_unpublished(
                        address,
                        thread_pointer_teb_offset,
                        scratch_teb_offset,
                        &unpublished,
                    )
                }?;
            }
        }
        unpublished.finish()?;
    }

    Ok(patched_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aot_cache_round_trip() {
        let temp_dir =
            std::env::temp_dir().join(format!("kinakaze_test_cache_{}", std::process::id()));
        let key = "test_object_1234.aot";

        let original = CachedObjectAot {
            segments: vec![CachedSegment {
                virtual_address: 0x400000,
                memory_size: 0x2000,
                canary_sites: vec![CachedCanarySite {
                    offset: 0x100,
                    len: 9,
                    prefix_offset: 0,
                    displacement_offset: 5,
                    displacement_size: 4,
                    original_displacement: 0x28,
                }],
                syscall_sites: vec![CachedSyscallSite {
                    offset: 0x250,
                    len: 2,
                    trampoline: true,
                }],
                thread_pointer_sites: vec![CachedThreadPointerSite {
                    offset: 0x300,
                    len: 9,
                }],
            }],
        };

        assert!(save_aot_cache(&temp_dir, key, &original).is_ok());
        let loaded = load_aot_cache(&temp_dir, key).expect("must load saved aot cache");

        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].virtual_address, 0x400000);
        assert_eq!(loaded.segments[0].memory_size, 0x2000);
        assert_eq!(loaded.segments[0].canary_sites.len(), 1);
        assert_eq!(
            loaded.segments[0].canary_sites[0].original_displacement,
            0x28
        );
        assert_eq!(loaded.segments[0].syscall_sites.len(), 1);
        assert_eq!(loaded.segments[0].syscall_sites[0].offset, 0x250);
        assert_eq!(loaded.segments[0].thread_pointer_sites.len(), 1);
        assert_eq!(loaded.segments[0].thread_pointer_sites[0].offset, 0x300);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn compute_cache_key_stable() {
        let data = b"\x7fELF\x02\x01\x01\x00test-binary-data";
        let k1 = compute_cache_key("test.so", data);
        let k2 = compute_cache_key("test.so", data);
        assert_eq!(k1, k2);
        assert!(k1.starts_with("test.so_"));
        assert!(k1.ends_with(".aot"));
    }

    #[test]
    fn cache_key_detects_middle_and_length_changes() {
        let original = vec![0x90; 32768];
        let key = compute_cache_key("lib/test.so", &original);
        let mut modified = original.clone();
        modified[16384] ^= 1;
        assert_ne!(key, compute_cache_key("lib/test.so", &modified));
        assert_ne!(key, compute_cache_key("lib/test.so", &original[..32767]));
        assert!(key.starts_with("lib_test.so_"));
        assert_eq!(key.len(), "lib_test.so_".len() + 64 + ".aot".len());
    }
}
