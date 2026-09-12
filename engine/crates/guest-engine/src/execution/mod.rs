//! Instruction translation and its disposable cache belong to the engine.
//! The dynamic linker lends an already relocated, writable image through runtime.
pub mod aot_cache;
mod code_roots;
pub(crate) mod guest_gs;
pub mod instruction_trampoline;
mod profile;
pub mod segment_patch;

use kinakaze_elf::{ElfFile, dynamic::DynamicInfo};
use kinakaze_runtime::execution::{CodeConfig, CodeImage, CodeResult};

#[derive(Debug)]
pub enum ExecutionError {
    AddressOverflow,
    MappingFailed { object: String, len: usize },
    ProtectionFailed { object: String },
    InvalidTls { object: String },
    InstructionPatch { address: usize, detail: String },
    Elf(kinakaze_elf::ElfError),
}

impl From<kinakaze_elf::ElfError> for ExecutionError {
    fn from(value: kinakaze_elf::ElfError) -> Self {
        Self::Elf(value)
    }
}

struct Image<'a> {
    bytes: &'a [u8],
    dynamic: DynamicInfo,
    view: &'a CodeImage,
}

impl Image<'_> {
    fn elf(&self) -> Result<ElfFile<'_>, ExecutionError> {
        Ok(ElfFile::parse(self.bytes)?)
    }

    fn resolve_address(&self, virtual_address: u64) -> Result<usize, ExecutionError> {
        let value = self
            .view
            .load_bias
            .checked_add(virtual_address as i128)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(ExecutionError::AddressOverflow)?;
        let end = self
            .view
            .base
            .checked_add(self.view.mapped_length)
            .ok_or(ExecutionError::AddressOverflow)?;
        if value < self.view.base || value >= end {
            return Err(ExecutionError::AddressOverflow);
        }
        Ok(value)
    }
}

/// Fixed native ABI; both the borrowed input and output storage belong to ld.so.
pub unsafe extern "C" fn prepare(
    view: *const CodeImage,
    config: *const CodeConfig,
    output: *mut CodeResult,
) -> i32 {
    if view.is_null() || config.is_null() || output.is_null() {
        return 22;
    }
    let (view, config, output) = unsafe { (&*view, &*config, &mut *output) };
    *output = CodeResult::default();
    match unsafe { prepare_image(view, config, output) } {
        Ok(()) => 0,
        Err(error) => {
            let message = format!("{error:?}");
            let length = message.len().min(output.error.len() - 1);
            output.error[..length].copy_from_slice(&message.as_bytes()[..length]);
            8
        }
    }
}

unsafe fn prepare_image(
    view: &CodeImage,
    config: &CodeConfig,
    output: &mut CodeResult,
) -> Result<(), ExecutionError> {
    if view.bytes.is_null()
        || view.length == 0
        || config.syscall_dispatcher == 0
        || config.canary_slot >= 64
    {
        return Err(ExecutionError::AddressOverflow);
    }
    let bytes = unsafe { std::slice::from_raw_parts(view.bytes, view.length) };
    let image = Image {
        bytes,
        dynamic: ElfFile::parse(bytes)?.dynamic_info()?.unwrap_or_default(),
        view,
    };
    let canary = segment_patch::teb_slot_offset(config.canary_slot);
    let slot = |value| (value < 64).then(|| segment_patch::teb_slot_offset(value));
    let thread = slot(config.thread_pointer_slot);
    let transition = slot(config.transition_slot);
    let scratch = slot(config.scratch_slot);
    let directory = super::CONFIGURATION
        .get()
        .expect("engine configured before image preparation")
        .root
        .join("var/cache/kinakaze/aot");
    let hashing = profile::begin("hash", bytes.len());
    let key = aot_cache::compute_cache_key("image", bytes);
    drop(hashing);
    let loading = profile::begin("cache-read", bytes.len());
    let cached = aot_cache::load_aot_cache(&directory, &key);
    drop(loading);
    if let Some(cached) = cached {
        // A cache hit is validated before any write. Once application starts,
        // a failure propagates; re-decoding partially rewritten bytes is unsafe.
        let validation = profile::begin("cache-validate", bytes.len());
        let valid = aot_cache::validate_cached_aot(&image, &cached, canary as u32).is_ok();
        drop(validation);
        if valid {
            let _application = profile::begin("cache-apply", bytes.len());
            output.canary_sites = aot_cache::apply_cached_aot(
                &image,
                &cached,
                canary as u32,
                Some(config.syscall_dispatcher),
                thread,
                transition,
                scratch,
            )?;
            output.syscall_sites = cached
                .segments
                .iter()
                .map(|segment| segment.syscall_sites.len())
                .sum();
            return Ok(());
        }
    }
    let scanning = profile::begin("decode-patch", bytes.len());
    let (report, cached) = segment_patch::retarget_thread_pointer_reads(
        &image,
        canary,
        thread,
        transition,
        scratch,
        segment_patch::AotSyscallPolicy::Trampoline(config.syscall_dispatcher),
    )?;
    drop(scanning);
    output.canary_sites = report.canary_sites;
    output.syscall_sites = report.syscall_sites.len();
    output.thread_pointer_sites = report.other_sites.len();
    // Caches are disposable performance data; a read-only root still runs the
    // fully prepared image. No execution error is suppressed here.
    let _ = aot_cache::save_aot_cache(&directory, &key, &cached);
    Ok(())
}
