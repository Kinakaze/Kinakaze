use crate::{Result, invalid};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const MAX_MODULES: usize = 128;
pub const MAX_EXPORTS: usize = 16 * 1024;
pub const MAX_VERSIONED_SYMBOLS: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExportKind {
    #[default]
    Function,
    Object,
}

/// Stateless images need no injected management table or fork rebinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleLifecycle {
    None,
    RuntimeApiV1,
}

#[derive(Clone)]
pub struct ModuleSet {
    pub modules: Vec<Module>,
    pub shared_libraries: Vec<String>,
    pub(crate) libraries: Vec<std::sync::Arc<kinakaze_v2_host_win::Library>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub id: u32,
    pub soname: String,
    pub lifecycle: ModuleLifecycle,
    pub exports: Vec<Export>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    pub pe_export: String,
    pub kind: ExportKind,
    /// Size of the actual exported object, required for STT_OBJECT.
    pub size: u64,
    pub alignment: u64,
    /// Exact ABI versions supplied by this implementation; never a wildcard.
    pub versions: Vec<String>,
    pub default_version: Option<String>,
}

impl Export {
    pub fn function(name: impl Into<String>, pe_export: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pe_export: pe_export.into(),
            kind: ExportKind::Function,
            size: 0,
            alignment: 1,
            versions: Vec::new(),
            default_version: None,
        }
    }

    pub fn supports_version(&self, version: Option<&str>) -> bool {
        match version {
            Some(version) => self.versions.iter().any(|candidate| candidate == version),
            None => self.versions.is_empty() || self.default_version.is_some(),
        }
    }
}

impl ModuleSet {
    /// Discover native images by their normal PE export table; no sidecar files.
    pub fn discover(directory: &Path) -> Result<Self> {
        crate::native::discover(directory)
    }

    pub fn validate(&self) -> Result<()> {
        if self.modules.is_empty() || self.modules.len() > MAX_MODULES {
            return Err(invalid("module set must have 1..=128 modules"));
        }
        let mut shared = HashSet::new();
        for library in &self.shared_libraries {
            validate_filename(library)?;
            if !shared.insert(library.to_ascii_lowercase()) {
                return Err(invalid("invalid or duplicate shared native library"));
            }
        }
        let mut ids = HashSet::new();
        let mut sonames = HashSet::new();
        for module in &self.modules {
            module.validate()?;
            if !ids.insert(module.id) {
                return Err(invalid(format!("duplicate module id {}", module.id)));
            }
            // Files are published on Windows: case-only differences collide.
            if !sonames.insert(module.soname.to_ascii_lowercase()) {
                return Err(invalid(format!("duplicate SONAME {}", module.soname)));
            }
        }
        if !self.modules.iter().any(|module| module.id == 0) {
            return Err(invalid("module set must include runtime module id 0"));
        }
        Ok(())
    }
}

impl Module {
    /// Real image registered with the guest linker. PE implementations live
    /// beside their native dependencies and use their SONAME as the filename.
    pub fn image_path(&self, dist: &Path) -> std::path::PathBuf {
        dist.join("rootfs/lib").join(&self.soname)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id == 0 && self.lifecycle != ModuleLifecycle::None {
            return Err(invalid(
                "runtime owns its session and cannot require provider initialization",
            ));
        }
        validate_filename(&self.soname)?;
        if !(self.soname.ends_with(".so") || self.soname.contains(".so.")) {
            return Err(invalid("module SONAME must have .so or .so.VERSION suffix"));
        }
        if self.exports.is_empty() || self.exports.len() > MAX_EXPORTS {
            return Err(invalid("module must export 1..=16384 symbols"));
        }
        let mut names = HashSet::new();
        let mut pe_shapes = HashMap::new();
        let mut versioned_symbols = 0;
        for export in &self.exports {
            validate_symbol(&export.name)?;
            validate_symbol(&export.pe_export)?;
            if !names.insert(&export.name) {
                return Err(invalid(format!(
                    "duplicate native export in {}",
                    self.soname
                )));
            }
            if !export.alignment.is_power_of_two() || export.alignment > 65536 {
                return Err(invalid(
                    "export alignment must be a power of two up to 65536",
                ));
            }
            match export.kind {
                ExportKind::Function if export.size != 0 || export.alignment != 1 => {
                    return Err(invalid(
                        "function exports must use default size and alignment",
                    ));
                }
                ExportKind::Object if export.size == 0 || export.size > 64 * 1024 * 1024 => {
                    return Err(invalid("object export size must be 1..=64 MiB"));
                }
                _ => {}
            }
            let shape = (export.kind, export.size, export.alignment);
            if let Some(previous) = pe_shapes.insert(&export.pe_export, shape)
                && previous != shape
            {
                return Err(invalid(
                    "aliases of one PE export must agree on symbol kind, size and alignment",
                ));
            }
            let mut versions = HashSet::new();
            if export.versions.len() > 64 {
                return Err(invalid("too many ABI versions per symbol"));
            }
            for version in &export.versions {
                validate_version(version)?;
                if !versions.insert(version) {
                    return Err(invalid("duplicate ABI version"));
                }
            }
            if let Some(default) = &export.default_version
                && !versions.contains(default)
            {
                return Err(invalid("default_version must appear in versions"));
            }
            versioned_symbols += export.versions.len().max(1);
            if versioned_symbols > MAX_VERSIONED_SYMBOLS {
                return Err(invalid("too many total versioned symbols"));
            }
        }
        Ok(())
    }
}

fn validate_version(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphabetic()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
    {
        return Err(invalid(format!("invalid explicit ABI version {name:?}")));
    }
    Ok(())
}

pub(crate) fn validate_filename(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.ends_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid(format!("unsafe module filename {name:?}")));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(invalid("reserved Windows device filename"));
    }
    Ok(())
}

fn validate_symbol(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !(name.as_bytes()[0].is_ascii_alphabetic() || name.starts_with('_'))
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(invalid(format!("invalid symbol name {name:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules() -> ModuleSet {
        super::test_modules()
    }

    #[test]
    fn duplicate_identity_and_path_collisions_are_rejected() {
        for variant in 0..2 {
            let mut value = modules();
            match variant {
                0 => value.modules[2].id = value.modules[1].id,
                _ => value.modules[2].soname = value.modules[1].soname.to_ascii_uppercase(),
            }
            assert!(value.validate().is_err());
        }
    }

    #[test]
    fn unsafe_paths_and_symbols_are_rejected() {
        for path in [
            "../x.dll",
            "C:/x.dll",
            "x\\a.dll",
            "CON.dll",
            "x.dll:stream",
            "x.dll.",
        ] {
            let mut value = modules();
            value.modules[1].soname = path.into();
            assert!(value.validate().is_err(), "{path}");
        }
        for name in ["", "x\0y", "9start", "a-b", "x@V1", "é"] {
            let mut value = modules();
            value.modules[1].exports[0].name = name.into();
            assert!(value.validate().is_err(), "{name}");
        }
    }

    #[test]
    fn aliases_versions_and_native_object_metadata_are_explicit() {
        let mut value = modules();
        value.modules[1].exports = vec![
            Export::function("getpid", "native_getpid"),
            Export::function("__getpid", "native_getpid"),
        ];
        value.validate().unwrap();
        let mut object = Export::function("environ", "native_environ");
        object.kind = ExportKind::Object;
        object.size = 8;
        object.alignment = 8;
        object.versions = vec!["GLIBC_2.2.5".into(), "GLIBC_2.34".into()];
        object.default_version = Some("GLIBC_2.2.5".into());
        assert!(object.supports_version(None));
        assert!(object.supports_version(Some("GLIBC_2.34")));
        assert!(!object.supports_version(Some("GLIBC_999")));
        let mut version_only = object.clone();
        version_only.default_version = None;
        assert!(!version_only.supports_version(None));
        assert!(version_only.supports_version(Some("GLIBC_2.2.5")));
        value.modules[1].exports.push(object);
        value.validate().unwrap();
        let mut bad_alias = value.clone();
        bad_alias.modules[1].exports[2].pe_export = "native_getpid".into();
        assert!(bad_alias.validate().is_err());
        for (versions, default) in [
            (vec!["GLIBC_*"], None),
            (vec!["GLIBC_2.2.5", "GLIBC_2.2.5"], None),
            (vec!["GLIBC_2.2.5"], Some("GLIBC_2.34")),
        ] {
            let mut bad = value.clone();
            bad.modules[1].exports[2].versions = versions.into_iter().map(str::to_owned).collect();
            bad.modules[1].exports[2].default_version = default.map(str::to_owned);
            assert!(bad.validate().is_err());
        }
        for (size, alignment) in [(0, 8), (8, 0), (8, 3), (u64::MAX, 8)] {
            let mut bad = value.clone();
            bad.modules[1].exports[2].size = size;
            bad.modules[1].exports[2].alignment = alignment;
            assert!(bad.validate().is_err());
        }
    }
}

#[cfg(test)]
pub(crate) fn test_modules() -> ModuleSet {
    ModuleSet {
        shared_libraries: Vec::new(),
        libraries: Vec::new(),
        modules: (0..3)
            .map(|id| Module {
                id,
                soname: format!("libtest{id}.so"),
                lifecycle: ModuleLifecycle::None,
                exports: vec![
                    Export::function("one", "native_one"),
                    Export::function("two", "native_two"),
                ],
            })
            .collect(),
    }
}
