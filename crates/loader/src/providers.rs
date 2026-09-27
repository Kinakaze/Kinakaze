//! Module binding and RAII ownership of linker images and fork registrations.
use crate::{CopyRedirect, handoff};
use guest_link::{ProviderImage, ProviderRegistry, ProviderSymbol, ProviderSymbolKind};
use kinakaze_v2_abi::{ProviderInitializeV1, RuntimeApiV1, STATUS_OK};
use kinakaze_v2_bridge::{
    ExportKind, ModuleLifecycle,
    native::{DiscoveredModule, ModuleCatalog},
};
use std::{
    ffi::{CString, c_void},
    mem,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

struct SharedModules {
    _images: Box<dyn Send + Sync>,
    libraries: Vec<kinakaze_v2_host_win::Library>,
}

impl SharedModules {
    fn load(dist: &Path, modules: &ModuleCatalog) -> Result<Self, Box<dyn std::error::Error>> {
        let _transaction =
            guest_process::begin_fork_mapping_transaction().ok_or("fork registry unavailable")?;
        let mut owner = Self {
            _images: Box::new(modules.retain_images()),
            libraries: Vec::with_capacity(modules.shared_libraries.len()),
        };
        let directory = kinakaze_v2_bridge::native::directory(dist);
        for name in &modules.shared_libraries {
            let library = kinakaze_v2_host_win::Library::open(&directory.join(name))?;
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
    image: mem::ManuallyDrop<Arc<kinakaze_v2_host_win::Library>>,
    _shared: Arc<SharedModules>,
    path: PathBuf,
    soname: String,
    symbols: Vec<ProviderSymbol>,
    bindings: Vec<NativeBinding>,
    module_base: usize,
    redirect: OnceLock<Result<CopyRedirect, String>>,
}

struct NativeBinding {
    name: String,
    alignment: usize,
    address: OnceLock<Result<usize, String>>,
}

impl NativeBinding {
    fn resolve(&self, image: &kinakaze_v2_host_win::Library) -> Result<usize, String> {
        let name = CString::new(self.name.as_str()).map_err(|error| error.to_string())?;
        // SAFETY: the owning image stays pinned for every published address.
        let address = unsafe { image.symbol(&name) }.map_err(|error| error.to_string())? as usize;
        if address == 0 || !address.is_multiple_of(self.alignment) {
            return Err(format!("invalid native symbol address: {}", self.name));
        }
        Ok(address)
    }
}

fn trace(soname: &str, symbol: Option<&str>) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_PROVIDER_TRACE").is_some()) {
        match symbol {
            Some(symbol) => eprintln!("kinakaze provider-symbol: {soname}:{symbol}"),
            None => eprintln!("kinakaze provider-load: {soname}"),
        }
    }
}

impl ProviderImage for BoundProvider {
    fn soname(&self) -> &str {
        &self.soname
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn canonical_path(&self) -> Option<&Path> {
        Some(self.image.path())
    }
    fn base(&self) -> usize {
        self.image.base_address()
    }
    fn symbols(&self) -> &[ProviderSymbol] {
        &self.symbols
    }
    fn has_deferred_addresses(&self) -> bool {
        true
    }
    fn symbol_address(&self, index: usize) -> Result<usize, guest_link::LinkError> {
        let binding = self.bindings.get(index).ok_or_else(|| {
            guest_link::LinkError::InvalidProvider("invalid native symbol index".into())
        })?;
        let address = binding.address.get_or_init(|| {
            trace(&self.soname, Some(&binding.name));
            binding.resolve(&self.image)
        });
        address.as_ref().copied().map_err(|error| {
            guest_link::LinkError::InvalidProvider(format!(
                "{}:{}: {error}",
                self.soname, binding.name
            ))
        })
    }
    fn inspect_symbol_address(&self, index: usize) -> Result<usize, guest_link::LinkError> {
        let binding = self.bindings.get(index).ok_or_else(|| {
            guest_link::LinkError::InvalidProvider("invalid native symbol index".into())
        })?;
        // dladdr searches by address, not by name. Its probes must not populate
        // bindings for unrelated exports encountered along the way.
        let address = match binding.address.get() {
            Some(address) => address.clone(),
            None => binding.resolve(&self.image),
        };
        address.map_err(guest_link::LinkError::InvalidProvider)
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
        if self.soname == "libc.so.6" {
            let redirect = self
                .redirect
                .get_or_init(|| {
                    // SAFETY: libc implements this fixed management ABI. Resolve
                    // it only when an actual COPY relocation needs the callback.
                    unsafe { self.image.symbol(c"kinakaze_copied_redirect") }
                        .map(|address| unsafe {
                            mem::transmute::<*mut c_void, CopyRedirect>(address)
                        })
                        .map_err(|error| error.to_string())
                })
                .as_ref()
                .map_err(|error| guest_link::LinkError::InvalidProvider(error.clone()))?;
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
    let modules = ModuleCatalog::discover(&kinakaze_v2_bridge::native::directory(dist))?;
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
    let directory = kinakaze_v2_bridge::native::directory(dist);
    for module in modules.modules {
        let path = directory.join(&module.soname);
        if module.lifecycle == ModuleLifecycle::RuntimeApiV1 {
            // Preserve mandatory initialization order; guest exports are still
            // unresolved until a relocation or explicit lookup requests them.
            registry.register(bind_module(path, module, Arc::clone(&shared), Some(api))?)?;
        } else {
            let shared = Arc::clone(&shared);
            registry.register_deferred(module.soname.clone(), path.clone(), move || {
                bind_module(path, module, shared, None)
                    .map_err(|error| guest_link::LinkError::InvalidProvider(error.to_string()))
            })?;
        }
    }
    Ok(Arc::new(registry))
}

fn bind_module(
    path: PathBuf,
    discovered: DiscoveredModule,
    shared: Arc<SharedModules>,
    api: Option<&RuntimeApiV1>,
) -> Result<Arc<dyn ProviderImage>, Box<dyn std::error::Error>> {
    let transaction =
        guest_process::begin_fork_mapping_transaction().ok_or("fork registry unavailable")?;
    // Metadata may load a DLL to query object layouts. Keep that first load,
    // registration and publication inside the same fork mapping transaction.
    let (module, library) = discovered.materialize()?;
    let soname = module.soname.clone();
    let lifecycle = module.lifecycle;
    let library = match library {
        Some(library) => library,
        None => Arc::new(kinakaze_v2_host_win::Library::open(&path)?),
    };
    trace(&soname, None);
    let exports = module.exports;
    let path = library.path().to_path_buf();
    let initialize = if lifecycle == ModuleLifecycle::RuntimeApiV1 {
        // SAFETY: each project provider implements this fixed native ABI.
        let initialize: ProviderInitializeV1 =
            unsafe { mem::transmute(library.symbol(c"kinakaze_provider_initialize_v1")?) };
        let api = api.ok_or("provider initialization requires a runtime API")?;
        let status = unsafe { initialize(api) };
        if status != STATUS_OK {
            return Err(format!("module {soname} initialization status {status}").into());
        }
        Some(initialize)
    } else {
        None
    };
    let mut symbols = Vec::with_capacity(exports.len());
    let mut bindings = Vec::with_capacity(exports.len());
    for export in exports {
        if !export.supports_version(None) {
            return Err(format!("module does not export {:?} at None", export.name).into());
        }
        let kind = match export.kind {
            ExportKind::Function => ProviderSymbolKind::Function,
            ExportKind::Object => ProviderSymbolKind::Object,
            ExportKind::Tls => {
                type ResolveTls =
                    unsafe extern "C" fn(*const u8, usize, *mut usize, *mut usize) -> bool;
                let resolve: ResolveTls =
                    unsafe { mem::transmute(library.symbol(c"kinakaze_module_tls_v1")?) };
                let (mut module, mut offset) = (0, 0);
                if !unsafe {
                    resolve(
                        export.name.as_ptr(),
                        export.name.len(),
                        &mut module,
                        &mut offset,
                    )
                } || module == 0
                {
                    return Err(format!("invalid native TLS symbol {}", export.name).into());
                }
                ProviderSymbolKind::Tls { module, offset }
            }
        };
        bindings.push(NativeBinding {
            name: export.pe_export,
            alignment: export.alignment as usize,
            address: OnceLock::new(),
        });
        symbols.push(ProviderSymbol {
            name: export.name,
            address: 0,
            kind,
            size: export.size,
            versions: export.versions,
            default_version: export.default_version,
        });
    }
    let module_base = library.base_address();
    if !guest_process::register_fork_module(module_base) {
        return Err("provider fork module registration failed".into());
    }
    let provider = BoundProvider {
        image: mem::ManuallyDrop::new(Arc::clone(&library)),
        _shared: Arc::clone(&shared),
        path,
        soname,
        symbols,
        bindings,
        module_base,
        redirect: OnceLock::new(),
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
    Ok(Arc::new(provider))
}
