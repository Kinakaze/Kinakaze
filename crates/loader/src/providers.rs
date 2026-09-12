//! Module binding and RAII ownership of linker images and fork registrations.
use crate::{CopyRedirect, handoff};
use guest_link::{ProviderImage, ProviderRegistry, ProviderSymbol, ProviderSymbolKind};
use kinakaze_v2_abi::{ProviderInitializeV1, RuntimeApiV1, STATUS_OK};
use kinakaze_v2_bridge::{ExportKind, ModuleImage, ModuleLifecycle, ModuleSet};
use std::{
    ffi::{CString, c_void},
    mem,
    path::{Path, PathBuf},
    sync::Arc,
};

struct SharedModules {
    libraries: Vec<kinakaze_v2_host_win::Library>,
}

impl SharedModules {
    fn load(dist: &Path, modules: &ModuleSet) -> Result<Self, Box<dyn std::error::Error>> {
        let _transaction =
            guest_process::begin_fork_mapping_transaction().ok_or("fork registry unavailable")?;
        let mut owner = Self {
            libraries: Vec::with_capacity(modules.shared_libraries.len()),
        };
        for name in &modules.shared_libraries {
            let library = kinakaze_v2_host_win::Library::open(&dist.join("rootfs/lib").join(name))?;
            if !guest_process::register_fork_module(library.base_address()) {
                return Err("shared native module registration failed".into());
            }
            owner.libraries.push(library);
        }
        Ok(owner)
    }
}

impl Drop for SharedModules {
    fn drop(&mut self) {
        let _transaction = guest_process::begin_fork_mapping_transaction()
            .unwrap_or_else(|| std::process::abort());
        for library in self.libraries.drain(..) {
            guest_process::unregister_fork_module(library.base_address());
            drop(library);
        }
    }
}

struct BoundProvider {
    image: mem::ManuallyDrop<ModuleImage>,
    _shared: Arc<SharedModules>,
    path: PathBuf,
    soname: String,
    symbols: Vec<ProviderSymbol>,
    module_base: usize,
    redirect: Option<CopyRedirect>,
}

impl ProviderImage for BoundProvider {
    fn soname(&self) -> &str {
        &self.soname
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn base(&self) -> usize {
        self.image.base_address()
    }
    fn symbols(&self) -> &[ProviderSymbol] {
        &self.symbols
    }
    fn mapped_len(&self) -> usize {
        self.image.mapped_len()
    }
    fn program_headers(&self) -> Option<(usize, u16)> {
        None
    }
    fn redirect_copy(
        &self,
        name: &str,
        address: usize,
        size: u64,
    ) -> Result<(), guest_link::LinkError> {
        let symbol = self
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .ok_or_else(|| guest_link::LinkError::InvalidProvider("unknown COPY symbol".into()))?;
        if size < symbol.size {
            return Err(guest_link::LinkError::InvalidProvider(format!(
                "undersized COPY object {name}"
            )));
        }
        if let Some(redirect) = self.redirect {
            let name = CString::new(name)
                .map_err(|_| guest_link::LinkError::InvalidSymbolName(name.into()))?;
            // SAFETY: linker supplies the checked, writable guest COPY storage.
            unsafe { redirect(name.as_ptr(), address as *mut c_void) };
        }
        Ok(())
    }
}

impl Drop for BoundProvider {
    fn drop(&mut self) {
        // Removing fork metadata and releasing the native mapping are one
        // transaction; another thread must not fork in the gap between them.
        let _transaction = guest_process::begin_fork_mapping_transaction()
            .unwrap_or_else(|| std::process::abort());
        handoff::remove(self.module_base);
        guest_process::unregister_fork_module(self.module_base);
        // SAFETY: Drop is the sole release path of this ManuallyDrop field.
        unsafe { mem::ManuallyDrop::drop(&mut self.image) };
    }
}

pub fn load(
    dist: &Path,
    api: &RuntimeApiV1,
) -> Result<Arc<ProviderRegistry>, Box<dyn std::error::Error>> {
    let _total = kinakaze_v2_host_win::StartupSpan::begin("providers-total");
    let discovery = kinakaze_v2_host_win::StartupSpan::begin("providers-discover");
    let modules = ModuleSet::discover(&dist.join("rootfs/lib"))?;
    drop(discovery);
    if !modules
        .modules
        .iter()
        .any(|module| module.soname == "libc.so.6")
    {
        return Err("ordinary workers require the self-developed libc provider".into());
    }
    let sharing = kinakaze_v2_host_win::StartupSpan::begin("providers-shared-load");
    let shared = Arc::new(SharedModules::load(dist, &modules)?);
    drop(sharing);
    let _binding = kinakaze_v2_host_win::StartupSpan::begin("providers-bind");
    let mut registry = ProviderRegistry::new();
    registry.set_restore_source(std::fs::canonicalize(dist)?);
    for module in &modules.modules {
        let transaction =
            guest_process::begin_fork_mapping_transaction().ok_or("fork registry unavailable")?;
        let path = module.image_path(dist);
        let image = ModuleImage::load(dist, module)
            .map_err(|error| format!("load {}: {error}", module.soname))?;
        let library = image.library();
        let initialize = if module.lifecycle == ModuleLifecycle::RuntimeApiV1 {
            // SAFETY: each project provider implements this fixed native ABI.
            let initialize: ProviderInitializeV1 =
                unsafe { mem::transmute(library.symbol(c"kinakaze_provider_initialize_v1")?) };
            let status = unsafe { initialize(api) };
            if status != STATUS_OK {
                return Err(
                    format!("module {} initialization status {status}", module.soname).into(),
                );
            }
            Some(initialize)
        } else {
            None
        };
        let mut symbols = Vec::with_capacity(module.exports.len());
        for export in &module.exports {
            symbols.push(ProviderSymbol {
                name: export.name.clone(),
                // SAFETY: only validated, bound addresses published to the linker.
                address: unsafe { image.symbol(&export.name)? } as usize,
                kind: match export.kind {
                    ExportKind::Function => ProviderSymbolKind::Function,
                    ExportKind::Object => ProviderSymbolKind::Object,
                },
                size: export.size,
                versions: export.versions.clone(),
                default_version: export.default_version.clone(),
            });
        }
        let module_base = library.base_address();
        if !guest_process::register_fork_module(module_base) {
            return Err("provider fork module registration failed".into());
        }
        let redirect = if module.soname == "libc.so.6" {
            // Fixed C ABI resolved from the owning image in each fresh worker.
            Some(unsafe {
                mem::transmute::<*mut c_void, CopyRedirect>(
                    library.symbol(c"kinakaze_copied_redirect")?,
                )
            })
        } else {
            None
        };
        let provider = BoundProvider {
            image: mem::ManuallyDrop::new(image),
            _shared: Arc::clone(&shared),
            path,
            soname: module.soname.clone(),
            symbols,
            module_base,
            redirect,
        };
        // Construct the registration owner first so a quota/lock failure also
        // unregisters the module and mapping before their native resources drop.
        if initialize.is_some() {
            handoff::insert(handoff::Binding {
                module_base,
                image_len: library.mapped_len(),
            })
            .map_err(|error| format!("provider handoff registration: {error}"))?;
        }
        drop(transaction);
        registry.register(Arc::new(provider))?;
    }
    Ok(Arc::new(registry))
}
