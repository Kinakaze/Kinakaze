//! A native module reference owns every address published to the guest linker.
use crate::{Export, Module, Result, invalid};
use kinakaze_v2_host_win::Library;
use std::{
    collections::HashMap,
    ffi::{CString, c_void},
    path::Path,
    sync::Arc,
};

pub struct ModuleImage {
    library: Arc<Library>,
    symbols: HashMap<String, BoundExport>,
}

struct BoundExport {
    declaration: Export,
    address: usize,
}

impl ModuleImage {
    pub fn load(dist: &Path, module: &Module) -> Result<Self> {
        module.validate()?;
        let library = Arc::new(Library::open(&module.image_path(dist))?);
        let mut symbols = HashMap::with_capacity(module.exports.len());
        for export in &module.exports {
            let name = CString::new(export.pe_export.as_str())
                .map_err(|_| invalid("invalid native symbol"))?;
            let address = unsafe { library.symbol(&name)? } as usize;
            if address == 0 || !address.is_multiple_of(export.alignment as usize) {
                return Err(invalid(format!(
                    "invalid native symbol address: {}",
                    export.name
                )));
            }
            symbols.insert(
                export.name.clone(),
                BoundExport {
                    declaration: export.clone(),
                    address,
                },
            );
        }
        Ok(Self { library, symbols })
    }

    pub fn library(&self) -> Arc<Library> {
        Arc::clone(&self.library)
    }
    pub fn base_address(&self) -> usize {
        self.library.base_address()
    }
    pub fn mapped_len(&self) -> usize {
        self.library.mapped_len()
    }

    /// # Safety
    /// Use the symbol's native ABI and retain this owner for the whole call.
    pub unsafe fn symbol(&self, name: &str) -> Result<*mut c_void> {
        unsafe { self.symbol_version(name, None) }
    }

    /// # Safety
    /// The requirements of `symbol` apply to exact version lookup too.
    pub unsafe fn symbol_version(&self, name: &str, version: Option<&str>) -> Result<*mut c_void> {
        let bound = self
            .symbols
            .get(name)
            .ok_or_else(|| invalid(format!("module does not export {name:?}")))?;
        let export = &bound.declaration;
        if !export.supports_version(version) {
            return Err(invalid(format!(
                "module does not export {name:?} at {version:?}"
            )));
        }
        // Load already resolved and validated the unversioned export while
        // pinning this library. Publishing it need not allocate/resolve again.
        let Some(version) = version else {
            return Ok(bound.address as *mut c_void);
        };
        let name = CString::new(format!("{}@{version}", export.pe_export))
            .map_err(|_| invalid("invalid native symbol"))?;
        Ok(unsafe { self.library.symbol(&name)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExportKind;

    #[test]
    fn renamed_pe_keeps_native_addresses_versions_and_lifetime() {
        let directory =
            std::env::temp_dir().join(format!("kinakaze-pe-owner-{}", std::process::id()));
        std::fs::create_dir_all(directory.join("rootfs/lib")).unwrap();
        let source = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/msvcrt.dll");
        let path = directory.join("rootfs/lib/libnative-test.so.1");
        std::fs::copy(source, &path).unwrap();
        let modules = crate::module::test_modules();
        let mut module = modules.modules[1].clone();
        module.soname = "libnative-test.so.1".into();
        let mut export = Export::function("timezone", "_timezone");
        export.kind = ExportKind::Object;
        export.size = 4;
        export.alignment = 4;
        module.exports = vec![export];
        let image = ModuleImage::load(&directory, &module).unwrap();
        assert!(image.mapped_len() > 0);
        let library = image.library();
        // SAFETY: compare addresses only; no Windows function is called with a
        // guest ABI, and both owners remain live for every lookup.
        unsafe {
            let address = library.symbol(c"_timezone").unwrap();
            assert_eq!(image.symbol("timezone").unwrap(), address);
            assert!(image.symbol_version("timezone", Some("TEST_2")).is_err());
            assert!(
                (image.base_address()..image.base_address() + image.mapped_len())
                    .contains(&(address as usize))
            );
        }
        assert!(kinakaze_v2_host_win::LoadedModule::pin(0).is_err());
        assert!(kinakaze_v2_host_win::LoadedModule::pin(image.base_address() + 1).is_err());
        let pinned = kinakaze_v2_host_win::LoadedModule::pin(image.base_address()).unwrap();
        assert_eq!(pinned.mapped_len(), image.mapped_len());
        let address = unsafe { image.symbol("timezone") }.unwrap();
        drop(image);
        // A retained library reference still pins its exports after image drop.
        assert!(unsafe { library.symbol(c"_timezone") }.is_ok());
        drop(library);
        // The child restore path owns an independent native reference, without
        // relying on the original image or its Rust owner having survived.
        assert_eq!(unsafe { pinned.symbol(c"_timezone") }.unwrap(), address);
        assert!(std::fs::remove_file(&path).is_err());
        drop(pinned);
        std::fs::remove_file(&path).unwrap();
        assert!(ModuleImage::load(&directory, &module).is_err());
        std::fs::remove_dir(directory.join("rootfs/lib")).unwrap();
        std::fs::remove_dir(directory.join("rootfs")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
