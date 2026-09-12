//! Relocation processing for x86_64.
//!
//! Relocations come from `DT_RELA`, `DT_JMPREL` and `DT_RELR` — never from
//! section headers. The distinction matters beyond stripping: the section-header
//! view also sweeps up relocations for debug sections that were never loaded, and
//! applying those writes to addresses that have no mapping.
//!
//! Every relocation is applied eagerly. Lazy PLT binding exists to shorten
//! startup, but it needs a resolver trampoline reached through the GOT, and doing
//! it eagerly is both simpler and strictly more predictable.

use kinakaze_elf::{
    ElfFile, R_X86_64_8, R_X86_64_16, R_X86_64_32, R_X86_64_32S, R_X86_64_64, R_X86_64_COPY,
    R_X86_64_DTPMOD64, R_X86_64_DTPOFF32, R_X86_64_DTPOFF64, R_X86_64_GLOB_DAT, R_X86_64_IRELATIVE,
    R_X86_64_JUMP_SLOT, R_X86_64_NONE, R_X86_64_PC8, R_X86_64_PC16, R_X86_64_PC32, R_X86_64_PC64,
    R_X86_64_PLT32, R_X86_64_RELATIVE, R_X86_64_RELATIVE64, R_X86_64_SIZE32, R_X86_64_SIZE64,
    R_X86_64_TPOFF32, R_X86_64_TPOFF64, Rela,
};

use crate::LinkError;
use crate::object::{MappedObject, ObjectId};
use crate::scope::{Resolution, Scope};

/// Reads `DT_RELA` and `DT_JMPREL` from the dynamic tables.
pub fn dynamic_relocations(object: &MappedObject) -> Result<Vec<Rela>, LinkError> {
    let mut result = Vec::new();
    visit_dynamic_relocations(object, |relocation| {
        result.push(relocation);
        Ok(())
    })?;
    Ok(result)
}

/// Visits dynamic relocations directly from the retained ELF image.
///
/// Large DSOs can carry hundreds of thousands of relative relocations. Linking
/// consumes each entry exactly once, so materializing the whole table first only
/// adds a multi-megabyte allocation and a second memory pass.
fn visit_dynamic_relocations(
    object: &MappedObject,
    mut visit: impl FnMut(Rela) -> Result<(), LinkError>,
) -> Result<(), LinkError> {
    let elf = object.elf()?;
    for table in [object.dynamic.rela, object.dynamic.jmprel]
        .into_iter()
        .flatten()
    {
        let entry_size = table.entry_size.unwrap_or(24).max(24) as usize;
        let Some(count) = table.count() else {
            continue;
        };
        let count = usize::try_from(count).map_err(|_| LinkError::AddressOverflow)?;
        let table_length = count
            .checked_mul(entry_size)
            .ok_or(LinkError::AddressOverflow)?;
        let entries = elf.slice(table.file_offset, table_length)?;
        for entry in entries.chunks_exact(entry_size) {
            let entry = &entry[..24];
            let info = u64::from_le_bytes([
                entry[8], entry[9], entry[10], entry[11], entry[12], entry[13], entry[14],
                entry[15],
            ]);
            visit(Rela {
                offset: u64::from_le_bytes([
                    entry[0], entry[1], entry[2], entry[3], entry[4], entry[5], entry[6], entry[7],
                ]),
                symbol_index: (info >> 32) as u32,
                kind: info as u32,
                addend: i64::from_le_bytes([
                    entry[16], entry[17], entry[18], entry[19], entry[20], entry[21], entry[22],
                    entry[23],
                ]),
            })?;
        }
    }
    Ok(())
}

/// Applies `DT_RELR`, the compressed encoding for relative relocations.
///
/// The format alternates: an even word is an address to relocate and becomes the
/// cursor; an odd word is a bitmap where each set bit relocates one word at a
/// stride from that cursor. It exists because a PIE's relative relocations are
/// dense and repetitive, and it shrinks them by roughly an order of magnitude.
pub fn apply_relr(object: &MappedObject) -> Result<usize, LinkError> {
    let Some(table) = object.dynamic.relr else {
        return Ok(0);
    };
    let Some(count) = table.count() else {
        return Ok(0);
    };
    let elf = object.elf()?;
    let count = usize::try_from(count).map_err(|_| LinkError::AddressOverflow)?;
    let table_length = count.checked_mul(8).ok_or(LinkError::AddressOverflow)?;
    let entries = elf.slice(table.file_offset, table_length)?;
    let mut applied = 0usize;
    let mut cursor = 0u64;
    for entry in entries.chunks_exact(8) {
        let word = u64::from_le_bytes([
            entry[0], entry[1], entry[2], entry[3], entry[4], entry[5], entry[6], entry[7],
        ]);

        if word & 1 == 0 {
            // An even entry is an address; relocate it and arm the cursor.
            relocate_relative(object, word)?;
            applied += 1;
            cursor = word.checked_add(8).ok_or(LinkError::AddressOverflow)?;
            continue;
        }

        // An odd entry is a bitmap covering the 63 words after the cursor.
        let mut bits = word >> 1;
        let mut position = cursor;
        while bits != 0 {
            if bits & 1 != 0 {
                relocate_relative(object, position)?;
                applied += 1;
            }
            bits >>= 1;
            position = position.checked_add(8).ok_or(LinkError::AddressOverflow)?;
        }
        cursor = cursor
            .checked_add(63 * 8)
            .ok_or(LinkError::AddressOverflow)?;
    }
    Ok(applied)
}

/// Adds the load bias to the word already stored at a virtual address.
fn relocate_relative(object: &MappedObject, virtual_address: u64) -> Result<(), LinkError> {
    let target = object.resolve_address(virtual_address)? as *mut u64;
    // SAFETY: the address lies in this object's writable mapping, and RELR only
    // ever names word-sized slots the linker placed.
    let existing = unsafe { target.read_unaligned() };
    let value = object
        .load_bias
        .checked_add(existing as i128)
        .ok_or(LinkError::AddressOverflow)?;
    let value = u64::try_from(value).map_err(|_| LinkError::AddressOverflow)?;
    // SAFETY: same slot, still writable at this point in the link.
    unsafe { target.write_unaligned(value) };
    Ok(())
}

/// Everything one relocation pass needs.
pub struct Context<'a> {
    pub objects: &'a [MappedObject],
    pub scope: &'a Scope,
    /// The object being relocated.
    pub owner: ObjectId,
    /// Resolver for the handful of symbols the host provides directly.
    pub builtins: &'a dyn Fn(&str) -> Option<usize>,
    /// What to do about a symbol nothing defines.
    pub unresolved_policy: crate::trap::UnresolvedPolicy,
    /// Variables the guest took its own copy of, collected as they are copied.
    ///
    /// Interior mutability because a `Context` is shared across the relocation of one
    /// object, and threading a `&mut` through every relocation kind would touch every
    /// arm to serve one of them.
    pub copies: &'a CopiedVariables,
    /// Direct TEB slot holding this thread's real Linux thread pointer. Host
    /// function resolutions are wrapped when this is present.
    pub thread_pointer_teb_slot: Option<u32>,
}

/// One `R_X86_64_COPY` that was performed, and where the guest's copy lives.
#[derive(Clone, Debug)]
pub struct CopiedVariable {
    /// The bare Linux symbol name.
    pub name: String,
    /// The address of the guest's own storage — the copy destination.
    pub guest_address: usize,
    pub source_address: usize,
    pub size: u64,
}

/// The copies performed during a link.
///
/// A `RefCell` rather than a `&mut Vec` so that [`Context`] can stay shared. Only one
/// thread relocates, so no lock is needed.
#[derive(Debug, Default)]
pub struct CopiedVariables(std::cell::RefCell<Vec<CopiedVariable>>);

impl CopiedVariables {
    /// Records one performed copy.
    pub fn push(&self, copy: CopiedVariable) {
        self.0.borrow_mut().push(copy);
    }

    /// Every copy recorded so far.
    pub fn entries(&self) -> Vec<CopiedVariable> {
        self.0.borrow().clone()
    }
}

/// A deferred `R_X86_64_IRELATIVE` or ifunc binding.
///
/// Resolvers must not run until every object is relocated and protected: a
/// resolver is ordinary code that can call other functions and read relocated
/// data, so running it mid-link would execute against a half-built image.
#[derive(Clone, Copy, Debug)]
pub struct PendingIfunc {
    pub target: usize,
    pub resolver: usize,
}

/// Applies every relocation in one object.
pub fn relocate_object(
    context: &Context<'_>,
    pending_ifuncs: &mut Vec<PendingIfunc>,
) -> Result<(), LinkError> {
    let object = &context.objects[context.owner.0];
    // RELR first: it is pure self-relocation with no symbol lookups, and the
    // ordering keeps it independent of anything the scope might resolve.
    apply_relr(object)?;

    let symbolic = object.dynamic.is_symbolic();
    visit_dynamic_relocations(object, |relocation| {
        apply_one(context, object, relocation, symbolic, pending_ifuncs)
    })
}

fn apply_one(
    context: &Context<'_>,
    object: &MappedObject,
    relocation: Rela,
    symbolic: bool,
    pending_ifuncs: &mut Vec<PendingIfunc>,
) -> Result<(), LinkError> {
    let kind = relocation.kind;
    if kind == R_X86_64_NONE {
        return Ok(());
    }
    let target = object.resolve_address(relocation.offset)?;

    // The self-relative forms need no symbol at all.
    match kind {
        R_X86_64_RELATIVE | R_X86_64_RELATIVE64 => {
            let value = object
                .load_bias
                .checked_add(relocation.addend as i128)
                .ok_or(LinkError::AddressOverflow)?;
            write_u64(
                target,
                u64::try_from(value).map_err(|_| LinkError::AddressOverflow)?,
            );
            return Ok(());
        }
        R_X86_64_IRELATIVE => {
            // The addend is the resolver's address. It is called later, once the
            // whole image is relocated and protected.
            let resolver = object
                .load_bias
                .checked_add(relocation.addend as i128)
                .ok_or(LinkError::AddressOverflow)?;
            pending_ifuncs.push(PendingIfunc {
                target,
                resolver: usize::try_from(resolver).map_err(|_| LinkError::AddressOverflow)?,
            });
            return Ok(());
        }
        _ => {}
    }

    // Everything else names a symbol.
    let Some(symbol) = object.symbol(relocation.symbol_index)? else {
        return Err(LinkError::UnresolvedSymbol {
            symbol: format!("<dynsym index {}>", relocation.symbol_index),
            version: None,
            requested_by: object.name.clone(),
        });
    };

    // A TLS module relocation with no symbol refers to the object itself, which is
    // how the local-dynamic model asks for its own module id.
    if kind == R_X86_64_DTPMOD64 && relocation.symbol_index == 0 {
        let module = object.tls_module.ok_or_else(|| LinkError::InvalidTls {
            object: object.name.clone(),
        })?;
        write_u64(target, module as u64);
        return Ok(());
    }

    let version = object.requirement_version(relocation.symbol_index);
    // STN_UNDEF with a local TLS relocation identifies this module's TLS block,
    // not an external symbol whose name happens to be empty (jemalloc uses it).
    let resolution = if relocation.symbol_index == 0
        && matches!(
            kind,
            R_X86_64_DTPOFF64 | R_X86_64_DTPOFF32 | R_X86_64_TPOFF64 | R_X86_64_TPOFF32
        ) {
        Resolution {
            owner: Some(context.owner),
            address: None,
            size: 0,
            tls_module: object.tls_module,
            tls_offset: Some(0),
            is_ifunc: false,
            is_trap: false,
        }
    } else {
        resolve_for(context, object, symbol, version, symbolic)?
    };

    match kind {
        R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => {
            let address = require_address(&resolution, symbol.name, object)?;
            // An ifunc reached through a normal data or PLT slot still needs its
            // resolver run before the slot is meaningful.
            if resolution.is_ifunc {
                pending_ifuncs.push(PendingIfunc {
                    target,
                    resolver: address,
                });
                return Ok(());
            }
            let value = add_signed(address as i128, relocation.addend)?;
            write_u64(target, value as u64);
        }
        R_X86_64_PC64 => {
            let address = require_address(&resolution, symbol.name, object)?;
            let value = address as i128 + relocation.addend as i128 - target as i128;
            write_u64(target, value as u64);
        }
        R_X86_64_PC32 | R_X86_64_PLT32 => {
            let address = require_address(&resolution, symbol.name, object)?;
            let value = address as i128 + relocation.addend as i128 - target as i128;
            let value = i32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_i32(target, value);
        }
        R_X86_64_32 => {
            let address = require_address(&resolution, symbol.name, object)?;
            let value = add_signed(address as i128, relocation.addend)?;
            let value = u32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_u32(target, value);
        }
        R_X86_64_32S => {
            let address = require_address(&resolution, symbol.name, object)?;
            let value = address as i128 + relocation.addend as i128;
            let value = i32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_i32(target, value);
        }
        R_X86_64_16 | R_X86_64_PC16 | R_X86_64_8 | R_X86_64_PC8 => {
            // Narrow forms only appear in hand-written or non-PIC code. They are
            // implemented for completeness rather than because a normal toolchain
            // emits them into a shared object.
            let address = require_address(&resolution, symbol.name, object)?;
            let mut value = address as i128 + relocation.addend as i128;
            if matches!(kind, R_X86_64_PC16 | R_X86_64_PC8) {
                value -= target as i128;
            }
            let overflow = || LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            };
            if matches!(kind, R_X86_64_16 | R_X86_64_PC16) {
                let narrow = i16::try_from(value).map_err(|_| overflow())?;
                // SAFETY: this relocation owns a two-byte slot in the image.
                unsafe { (target as *mut i16).write_unaligned(narrow) };
            } else {
                let narrow = i8::try_from(value).map_err(|_| overflow())?;
                // SAFETY: this relocation owns a one-byte slot in the image.
                unsafe { (target as *mut i8).write_unaligned(narrow) };
            }
        }
        R_X86_64_COPY => {
            // The executable holds its own storage for a library's variable, and
            // the library's initial value has to be copied into it. This is why
            // `stdout` and `environ` work when referenced from a main program.
            // Re-resolve, excluding this object. The general resolution above found
            // this object's own storage, which is the copy *destination*: see
            // `Scope::resolve_excluding` for why that has to be skipped.
            let source = context.scope.resolve_excluding(
                context.objects,
                symbol.name,
                version,
                Some(context.owner),
            )?;
            let Some(resolution) = source else {
                // Nothing outside this object defines it, so there is no initial
                // value to copy. The destination keeps its zeroes.
                if context.unresolved_policy == crate::trap::UnresolvedPolicy::Trap {
                    crate::trap::trap_for(symbol.name, false)?;
                    return Ok(());
                }
                return Err(LinkError::BadCopyRelocation {
                    symbol: symbol.name.to_owned(),
                });
            };
            // A trapped symbol has no value to copy: its address is a guard page,
            // and reading through it is precisely the fault the trap exists to
            // cause. The destination keeps the zeroes the mapping started with,
            // which is what an uninitialized copy of a missing variable should look
            // like, and the name is already recorded as trapped so the gap stays
            // reportable rather than silent.
            if resolution.is_trap {
                return Ok(());
            }
            let address = require_address(&resolution, symbol.name, object)?;
            // COPY has two independently sized allocations. Never read past
            // the provider or write past the executable when their ABI differs.
            let size = usize::try_from(resolution.size.min(symbol.size))
                .map_err(|_| LinkError::AddressOverflow)?;
            // A zero address is what an undefined weak symbol resolves to. That is
            // a legitimate resolution for a *reference*, but there is nothing at
            // address zero to copy a value out of, so it has to be caught here
            // rather than dereferenced.
            if size == 0 || address == 0 {
                return Err(LinkError::BadCopyRelocation {
                    symbol: symbol.name.to_owned(),
                });
            }
            if std::env::var_os("KINAKAZE_TRACE_COPIES").is_some() {
                let preview_length = size.min(4 * std::mem::size_of::<usize>());
                // SAFETY: the source range was validated by symbol resolution;
                // this diagnostic only reads a prefix within its declared size.
                let preview =
                    unsafe { core::slice::from_raw_parts(address as *const u8, preview_length) };
                eprintln!(
                    "kinakaze: COPY {} owner={} source={:#x} target={:#x} size={} bytes={:02x?}",
                    symbol.name, object.name, address, target, size, preview
                );
            }
            // SAFETY: the source is a real definition in a mapped image (traps were
            // rejected above) and the destination is this object's own writable
            // slot, both at least `size` bytes.
            unsafe { std::ptr::copy_nonoverlapping(address as *const u8, target as *mut u8, size) };

            // Copying the value is only half of what the ABI requires. The provider's
            // *own* accesses must also reach this new location, or the two copies
            // diverge the moment either side writes.
            //
            // On Linux that happens for free: the provider is a PIC shared object whose
            // accesses go through its GOT, and the dynamic linker repoints that entry
            // here. A Windows DLL's accesses are direct, RIP-relative, and unreachable
            // by any relocation — so the provider has to be *told*. Recorded here and
            // applied by `Linker::install_copy_redirects` once relocation is complete.
            //
            // Skipping this is a quiet, confusing failure: `busybox ls` parsed `-l`
            // correctly and then listed a file called `ls`, because libc advanced its
            // own `optind` while BusyBox read the copy in its own `.bss`.
            context.copies.push(CopiedVariable {
                name: symbol.name.to_owned(),
                guest_address: target as usize,
                source_address: address,
                size: symbol.size,
            });
        }
        R_X86_64_SIZE32 => {
            let value = add_signed(resolution.size as i128, relocation.addend)?;
            let value = u32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_u32(target, value);
        }
        R_X86_64_SIZE64 => {
            let value = add_signed(resolution.size as i128, relocation.addend)?;
            write_u64(target, value as u64);
        }
        R_X86_64_DTPMOD64 => {
            let module = resolution.tls_module.or(object.tls_module).ok_or_else(|| {
                LinkError::InvalidTls {
                    object: object.name.clone(),
                }
            })?;
            write_u64(target, module as u64);
        }
        R_X86_64_DTPOFF64 => {
            let offset = resolution.tls_offset.ok_or_else(|| LinkError::InvalidTls {
                object: object.name.clone(),
            })?;
            let value = add_signed(offset as i128, relocation.addend)?;
            write_u64(target, value as u64);
        }
        R_X86_64_DTPOFF32 => {
            let offset = resolution.tls_offset.ok_or_else(|| LinkError::InvalidTls {
                object: object.name.clone(),
            })?;
            let value = add_signed(offset as i128, relocation.addend)?;
            let value = u32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_u32(target, value);
        }
        R_X86_64_TPOFF64 => {
            let module = resolution.tls_module.or(object.tls_module).ok_or_else(|| {
                LinkError::InvalidTls {
                    object: object.name.clone(),
                }
            })?;
            let module_offset = kinakaze_tls::reserve_static_elf_module(module).map_err(|_| {
                LinkError::InvalidTls {
                    object: object.name.clone(),
                }
            })?;
            let offset = resolution.tls_offset.unwrap_or(symbol.value as usize);
            let value = add_signed(offset as i128 - module_offset as i128, relocation.addend)?;
            let value = i64::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_u64(target, value as u64);
        }
        R_X86_64_TPOFF32 => {
            let module = resolution.tls_module.or(object.tls_module).ok_or_else(|| {
                LinkError::InvalidTls {
                    object: object.name.clone(),
                }
            })?;
            let module_offset = kinakaze_tls::reserve_static_elf_module(module).map_err(|_| {
                LinkError::InvalidTls {
                    object: object.name.clone(),
                }
            })?;
            let offset = resolution.tls_offset.unwrap_or(symbol.value as usize);
            let value = add_signed(offset as i128 - module_offset as i128, relocation.addend)?;
            let value = i32::try_from(value).map_err(|_| LinkError::RelocationOverflow {
                kind,
                symbol: symbol.name.to_owned(),
            })?;
            write_i32(target, value);
        }
        other => {
            return Err(LinkError::UnsupportedRelocation {
                kind: other,
                object: object.name.clone(),
            });
        }
    }
    Ok(())
}

/// Resolves a relocation's symbol, honouring `DF_SYMBOLIC` and local bindings.
fn resolve_for(
    context: &Context<'_>,
    object: &MappedObject,
    symbol: kinakaze_elf::DynamicSymbol<'_>,
    version: Option<&str>,
    symbolic: bool,
) -> Result<Resolution, LinkError> {
    // The host provides a few symbols directly; they win outright because nothing
    // in the guest can implement them.
    if let Some(address) = (context.builtins)(symbol.name) {
        return wrap_host_function(context, symbol, Resolution::from_address(address));
    }

    // A definition that cannot be interposed is used as-is without consulting the
    // scope: that is what hidden visibility and local binding mean.
    if symbol.is_defined() && symbol.is_local_only() {
        return local_definition(object, context.owner, symbol);
    }

    let resolved = if symbolic {
        context
            .scope
            .resolve_symbolic(context.objects, context.owner, symbol.name, version)?
    } else {
        context
            .scope
            .resolve(context.objects, symbol.name, version)?
    };
    if let Some(resolved) = resolved {
        return wrap_host_function(context, symbol, resolved);
    }

    // A versioned lookup that found nothing may still be satisfiable without the
    // version. Providers that carry no version tables are already handled inside
    // the scope, so reaching here means the name itself was absent under that
    // version; retrying unversioned is what glibc does for a weak reference and it
    // keeps a mismatched version from being fatal.
    if version.is_some()
        && let Some(resolved) = context.scope.resolve(context.objects, symbol.name, None)?
    {
        return wrap_host_function(context, symbol, resolved);
    }

    // An object's own definition is the last resort before failing.
    if symbol.is_defined() {
        return local_definition(object, context.owner, symbol);
    }

    // An undefined weak symbol legitimately resolves to zero. That is the entire
    // mechanism behind optional symbols such as `__gmon_start__`.
    if symbol.binding() == kinakaze_elf::STB_WEAK {
        return Ok(Resolution {
            owner: None,
            address: Some(0),
            size: 0,
            tls_module: None,
            tls_offset: None,
            is_ifunc: false,
            is_trap: false,
        });
    }

    // Nothing defines it. Failing is right for a self-contained object, but for a
    // real binary it means one unused symbol blocks the whole program, so the
    // caller may ask for a trap instead. See `crate::trap`.
    if context.unresolved_policy == crate::trap::UnresolvedPolicy::Trap {
        // An object symbol is read through, not called, so it cannot trap by
        // executing; `trap_for` routes those to a guard page instead.
        let is_function = !matches!(symbol.symbol_type(), kinakaze_elf::STT_OBJECT);
        return Ok(Resolution::trap(crate::trap::trap_for(
            symbol.name,
            is_function,
        )?));
    }

    Err(LinkError::UnresolvedSymbol {
        symbol: symbol.name.to_owned(),
        version: version.map(str::to_owned),
        requested_by: object.name.clone(),
    })
}

fn wrap_host_function(
    context: &Context<'_>,
    symbol: kinakaze_elf::DynamicSymbol<'_>,
    resolution: Resolution,
) -> Result<Resolution, LinkError> {
    let _ = (context, symbol);
    Ok(resolution)
}

fn local_definition(
    object: &MappedObject,
    owner: ObjectId,
    symbol: kinakaze_elf::DynamicSymbol<'_>,
) -> Result<Resolution, LinkError> {
    if object.is_tls(symbol) {
        return Ok(Resolution {
            owner: Some(owner),
            address: None,
            size: symbol.size,
            tls_module: object.tls_module,
            tls_offset: Some(
                usize::try_from(symbol.value).map_err(|_| LinkError::AddressOverflow)?,
            ),
            is_ifunc: false,
            is_trap: false,
        });
    }
    Ok(Resolution {
        owner: Some(owner),
        address: Some(object.definition_address(symbol)?),
        size: symbol.size,
        tls_module: None,
        tls_offset: None,
        is_ifunc: object.is_ifunc(symbol),
        is_trap: false,
    })
}

fn require_address(
    resolution: &Resolution,
    name: &str,
    object: &MappedObject,
) -> Result<usize, LinkError> {
    resolution
        .address
        .ok_or_else(|| LinkError::UnresolvedSymbol {
            symbol: name.to_owned(),
            version: None,
            requested_by: object.name.clone(),
        })
}

/// Runs the deferred ifunc resolvers and stores their results.
///
/// # Safety
///
/// Every object must already be relocated and protected: a resolver is ordinary
/// code and may depend on the rest of the image being complete.
pub unsafe fn run_ifuncs(pending: &[PendingIfunc]) -> Result<(), LinkError> {
    for entry in pending {
        // SAFETY: the resolver address came from an IRELATIVE addend or an ifunc
        // symbol, both of which name a function with this signature by ABI.
        let resolver: unsafe extern "sysv64" fn() -> usize =
            unsafe { std::mem::transmute(entry.resolver) };
        // SAFETY: the caller guarantees the image is fully linked.
        let address = unsafe { resolver() };
        write_u64(entry.target, address as u64);
    }
    Ok(())
}

fn add_signed(base: i128, addend: i64) -> Result<i128, LinkError> {
    base.checked_add(addend as i128)
        .ok_or(LinkError::AddressOverflow)
}

fn write_u64(target: usize, value: u64) {
    // SAFETY: relocation targets are slots the linker owns in a writable mapping.
    unsafe { (target as *mut u64).write_unaligned(value) };
}

fn write_u32(target: usize, value: u32) {
    // SAFETY: as above, for a four-byte slot.
    unsafe { (target as *mut u32).write_unaligned(value) };
}

fn write_i32(target: usize, value: i32) {
    // SAFETY: as above, for a four-byte slot.
    unsafe { (target as *mut i32).write_unaligned(value) };
}

/// Reads relocation counts for diagnostics.
pub fn relocation_summary(object: &MappedObject) -> Result<(usize, usize), LinkError> {
    let elf = ElfFile::parse(&object.bytes)?;
    let _ = elf;
    let rela = object
        .dynamic
        .rela
        .and_then(|table| table.count())
        .unwrap_or(0) as usize;
    let plt = object
        .dynamic
        .jmprel
        .and_then(|table| table.count())
        .unwrap_or(0) as usize;
    Ok((rela, plt))
}
