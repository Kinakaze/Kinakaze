//! A dynamic linker for ELF objects hosted in a Windows process.
//!
//! This is the `ld.so` half of the project. It maps ELF shared objects, walks the
//! `DT_NEEDED` graph, builds the global symbol scope, applies relocations, lays
//! out TLS, runs initializers in dependency order, and hands out `dlopen`-style
//! handles.
//!
//! Two design points drive the rest:
//!
//! **Everything comes from `PT_DYNAMIC`.** Section headers are a link-time
//! artifact that real binaries frequently strip, and they are never guaranteed to
//! be mapped. A loader that reads `SHT_DYNSYM` works only on unstripped output
//! from a friendly toolchain.
//!
//! **Native PE and third-party ELF share one linker.** Native SONAMEs and exact
//! versioned exports come from the loaded images; third-party ELF libraries
//! retain their dynamic symbol, TLS and relocation semantics.

mod object_layout;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod process;

pub mod launch;
pub mod linker;
pub mod object;
pub mod provider;
pub mod relocate;
pub mod scope;
pub mod stack;
pub mod trap;

use std::path::PathBuf;

pub use kinakaze_runtime::immutable::ImmutableBytes;
pub use kinakaze_vfs::{open_guest_image, read_guest_image, snapshot_guest_image};
pub use linker::{Linker, SearchPaths};
pub use object::{MappedObject, ObjectId};
pub use provider::{ProviderImage, ProviderRegistry, ProviderSymbol, ProviderSymbolKind};
pub use scope::{Resolution, Scope};
pub use trap::UnresolvedPolicy;

/// Everything that can go wrong while linking.
#[derive(Debug)]
pub enum LinkError {
    Execution(String),
    /// A provider was unregistered or its explicit facade metadata was invalid.
    InvalidProvider(String),
    Io(std::io::Error),
    Elf(kinakaze_elf::ElfError),
    /// A `DT_NEEDED` entry matched neither a DLL nor an ELF object on disk.
    MissingDependency {
        name: String,
        requested_by: String,
        searched: Vec<PathBuf>,
    },
    /// `LoadLibraryW` failed for a mapped dependency.
    DllLoadFailed(PathBuf),
    /// A relocation referenced a symbol nothing defines.
    UnresolvedSymbol {
        symbol: String,
        version: Option<String>,
        requested_by: String,
    },
    /// A relocation type this linker does not implement.
    UnsupportedRelocation {
        kind: u32,
        object: String,
    },
    /// A relocation's computed value did not fit its field width.
    RelocationOverflow {
        kind: u32,
        symbol: String,
    },
    /// A symbol name contained an interior NUL and cannot cross the C boundary.
    InvalidSymbolName(String),
    /// `VirtualAlloc` could not place an image.
    MappingFailed {
        object: String,
        len: usize,
    },
    /// The image was mapped, but the fixed fork-clone registry was full.
    MappingRegistrationFailed {
        object: String,
        len: usize,
    },
    /// `VirtualProtect` failed while applying segment permissions.
    ProtectionFailed {
        object: String,
    },
    /// An address computation left the representable range.
    AddressOverflow,
    /// TLS registration failed or a TLS symbol had no module.
    InvalidTls {
        object: String,
    },
    /// The object has no usable entry point.
    NoEntryPoint {
        object: String,
    },
    /// Startup requested main-program initialization without a main object.
    NoExecutableObject,
    /// More than one object was marked as the process executable.
    MultipleExecutableObjects,
    /// libc attempted main initialization before the loader preinit phase.
    ExecutablePreinitNotRun,
    /// A `dlopen` handle was not one this linker produced.
    InvalidHandle,
    /// `dlopen` received an unsupported or contradictory mode.
    InvalidOpenFlags(i32),
    /// RTLD_NOLOAD named an object absent from the live link-map.
    ObjectNotLoaded(PathBuf),
    /// RTLD_NEXT was invoked from outside every loaded ELF object.
    InvalidCallerAddress(usize),
    /// A COPY relocation named a symbol with no size or no definition to copy.
    BadCopyRelocation {
        symbol: String,
    },
}

impl From<std::io::Error> for LinkError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<kinakaze_elf::ElfError> for LinkError {
    fn from(error: kinakaze_elf::ElfError) -> Self {
        Self::Elf(error)
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Execution(message) => write!(formatter, "code preparation failed: {message}"),
            Self::InvalidProvider(message) => write!(formatter, "invalid V2 provider: {message}"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Elf(error) => write!(formatter, "ELF parse error: {error:?}"),
            Self::MissingDependency {
                name,
                requested_by,
                searched,
            } => write!(
                formatter,
                "{requested_by} needs {name:?}, which was not found as a DLL or an ELF object; \
                 searched {}",
                searched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::DllLoadFailed(path) => {
                write!(formatter, "LoadLibraryW failed for {}", path.display())
            }
            Self::UnresolvedSymbol {
                symbol,
                version,
                requested_by,
            } => match version {
                Some(version) => write!(
                    formatter,
                    "unresolved symbol {symbol}@{version} referenced by {requested_by}"
                ),
                None => write!(
                    formatter,
                    "unresolved symbol {symbol} referenced by {requested_by}"
                ),
            },
            Self::UnsupportedRelocation { kind, object } => write!(
                formatter,
                "unsupported x86_64 relocation {kind} in {object}"
            ),
            Self::RelocationOverflow { kind, symbol } => write!(
                formatter,
                "relocation {kind} for {symbol} does not fit its field"
            ),
            Self::InvalidSymbolName(name) => {
                write!(formatter, "symbol name {name:?} contains an interior NUL")
            }
            Self::MappingFailed { object, len } => {
                write!(formatter, "VirtualAlloc failed for {object} ({len} bytes)")
            }
            Self::MappingRegistrationFailed { object, len } => write!(
                formatter,
                "fork mapping registry is full while loading {object} ({len} bytes)"
            ),
            Self::ProtectionFailed { object } => {
                write!(formatter, "VirtualProtect failed for a segment of {object}")
            }
            Self::AddressOverflow => write!(formatter, "an address computation overflowed"),
            Self::InvalidTls { object } => {
                write!(formatter, "invalid or unavailable TLS in {object}")
            }
            Self::NoEntryPoint { object } => {
                write!(formatter, "{object} has no valid entry point")
            }
            Self::NoExecutableObject => write!(formatter, "the link-map has no main executable"),
            Self::MultipleExecutableObjects => {
                write!(formatter, "the link-map has multiple main executables")
            }
            Self::ExecutablePreinitNotRun => write!(
                formatter,
                "main executable initialization began before its preinit phase"
            ),
            Self::InvalidHandle => write!(formatter, "not a handle produced by this linker"),
            Self::InvalidOpenFlags(flags) => write!(formatter, "invalid dlopen mode {flags:#x}"),
            Self::ObjectNotLoaded(path) => write!(formatter, "{} is not loaded", path.display()),
            Self::InvalidCallerAddress(address) => write!(
                formatter,
                "RTLD_NEXT caller {address:#x} is outside the ELF link-map"
            ),
            Self::BadCopyRelocation { symbol } => write!(
                formatter,
                "COPY relocation for {symbol} has no sized definition to copy from"
            ),
        }
    }
}

impl std::error::Error for LinkError {}
