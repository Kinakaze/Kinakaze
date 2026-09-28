//! Explicit, validated provider images. Ownership pins the native PE module
//! and, where still present, its ELF facade for every published guest address.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::LinkError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSymbolKind {
    Function,
    Object,
    Tls { module: usize, offset: usize },
}

#[derive(Clone, Debug)]
pub struct ProviderSymbol {
    pub name: String,
    /// Zero denotes an unresolved declaration only for adapters opting into
    /// deferred addresses. Consumers obtain addresses through symbol_address.
    pub address: usize,
    pub kind: ProviderSymbolKind,
    pub size: u64,
    /// Explicit ABI versions provided by this definition; never a wildcard.
    pub versions: Vec<String>,
    /// The version visible to unversioned lookup. None with nonempty versions
    /// means this definition is only accessible with an explicit version.
    pub default_version: Option<String>,
}

impl ProviderSymbol {
    pub fn matches_version(&self, version: Option<&str>) -> bool {
        match version {
            Some(version) => self.versions.iter().any(|item| item == version),
            None => self.versions.is_empty() || self.default_version.is_some(),
        }
    }
}

/// A trusted host adapter over a declared and bound provider image.
///
/// Implementations must retain their native module and any facade. Native PE
/// functions and objects expose their actual addresses; legacy facade functions
/// still expose thunks. Aliases and versions come from explicit ABI metadata.
pub trait ProviderImage: Send + Sync {
    fn soname(&self) -> &str;
    fn path(&self) -> &Path;
    /// Canonical file identity already obtained while opening this owned image.
    /// Adapters without a pinned canonical identity use the default filesystem
    /// lookup. This must identify the same file as `path()`.
    fn canonical_path(&self) -> Option<&Path> {
        None
    }
    fn base(&self) -> usize;
    fn symbols(&self) -> &[ProviderSymbol];
    /// An unbound address in metadata is permitted only when this adapter
    /// resolves and validates it on the first actual symbol lookup.
    fn has_deferred_addresses(&self) -> bool {
        false
    }
    fn symbol_address(&self, index: usize) -> Result<usize, LinkError> {
        self.symbols()
            .get(index)
            .map(|symbol| symbol.address)
            .filter(|address| *address != 0)
            .ok_or_else(|| LinkError::InvalidProvider("invalid provider symbol address".into()))
    }
    /// Reverse lookup may inspect declarations while searching for an address.
    /// Deferred adapters need not publish bindings for those incidental probes.
    fn inspect_symbol_address(&self, index: usize) -> Result<usize, LinkError> {
        self.symbol_address(index)
    }
    fn mapped_len(&self) -> usize {
        0
    }
    /// Genuine mapped ELF headers, if present. PE modules return None.
    fn program_headers(&self) -> Option<(usize, u16)> {
        None
    }

    /// Redirect provider accesses to the executable's COPY-relocation storage.
    /// Failure must be reported if the provider cannot preserve shared storage.
    fn redirect_copy(&self, name: &str, address: usize, size: u64) -> Result<(), LinkError>;
}

#[derive(Default)]
pub struct ProviderRegistry {
    images: HashMap<String, ProviderEntry>,
    paths: HashMap<std::path::PathBuf, String>,
    pub(crate) restore_source: Option<std::path::PathBuf>,
    interpreter_image: OnceLock<Option<crate::ImmutableBytes>>,
}

type Factory = Box<dyn FnOnce() -> Result<Arc<dyn ProviderImage>, LinkError> + Send>;

enum ProviderEntry {
    Ready(Arc<dyn ProviderImage>),
    Deferred {
        factory: Mutex<Option<Factory>>,
        image: OnceLock<Result<Arc<dyn ProviderImage>, String>>,
        path: std::path::PathBuf,
    },
}

impl ProviderEntry {
    fn path(&self) -> &Path {
        match self {
            Self::Ready(image) => image.path(),
            Self::Deferred { path, .. } => path,
        }
    }
    fn get(&self, name: &str) -> Result<Arc<dyn ProviderImage>, LinkError> {
        let Self::Deferred {
            factory,
            image,
            path,
        } = self
        else {
            let Self::Ready(image) = self else {
                unreachable!()
            };
            return Ok(Arc::clone(image));
        };
        // One initialization owns the factory, including on failure. Other
        // threads observe the same validated image or the same load error.
        let result = image.get_or_init(|| {
            let load = factory
                .lock()
                .map_err(|_| "provider factory poisoned".to_owned())?
                .take()
                .ok_or_else(|| "provider factory consumed".to_owned())?;
            let image = load().map_err(|error| error.to_string())?;
            if image.soname() != name || canonical_path(image.as_ref()) != *path {
                return Err("deferred provider changed its registered identity".into());
            }
            validate_symbols(image.as_ref()).map_err(|error| error.to_string())?;
            Ok(image)
        });
        match result {
            Ok(image) => Ok(Arc::clone(image)),
            Err(error) => Err(LinkError::InvalidProvider(format!("load {name}: {error}"))),
        }
    }
}

fn canonical_path(image: &dyn ProviderImage) -> std::path::PathBuf {
    image
        .canonical_path()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::fs::canonicalize(image.path()).unwrap_or_else(|_| image.path().to_path_buf())
        })
}

fn validate_symbols(image: &dyn ProviderImage) -> Result<(), LinkError> {
    let name = image.soname();
    let mut names = HashSet::new();
    for symbol in image.symbols() {
        if symbol.name.is_empty() || symbol.name.contains('\0') || !names.insert(&symbol.name) {
            return Err(LinkError::InvalidProvider(format!(
                "invalid or duplicate symbol in {name}"
            )));
        }
        if symbol.address == 0 && !image.has_deferred_addresses()
            || (symbol.kind != ProviderSymbolKind::Function && symbol.size == 0)
        {
            return Err(LinkError::InvalidProvider(format!(
                "invalid address or object size for {}",
                symbol.name
            )));
        }
        let mut versions = HashSet::new();
        if symbol.versions.iter().any(|version| {
            version.is_empty() || version.contains('\0') || !versions.insert(version)
        }) || symbol
            .default_version
            .as_ref()
            .is_some_and(|version| !versions.contains(version))
        {
            return Err(LinkError::InvalidProvider(format!(
                "invalid ABI versions for {}",
                symbol.name
            )));
        }
    }
    Ok(())
}

type RegistryLoader = fn(&Path) -> Result<Arc<ProviderRegistry>, LinkError>;
static REGISTRY_LOADER: std::sync::OnceLock<RegistryLoader> = std::sync::OnceLock::new();

/// The embedding loader installs this in each fresh worker before fork restore.
pub fn install_registry_loader(loader: RegistryLoader) -> Result<(), LinkError> {
    let installed = REGISTRY_LOADER.get_or_init(|| loader);
    if !std::ptr::fn_addr_eq(*installed, loader) {
        return Err(LinkError::InvalidProvider(
            "registry loader already installed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn restore_registry(source: &Path) -> Result<Arc<ProviderRegistry>, LinkError> {
    REGISTRY_LOADER
        .get()
        .ok_or_else(|| LinkError::InvalidProvider("registry loader unavailable".into()))?(source)
}

impl ProviderRegistry {
    pub fn set_restore_source(&mut self, source: std::path::PathBuf) {
        self.restore_source = Some(source);
    }
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one declared SONAME and one independently owned image.
    pub fn register(&mut self, image: Arc<dyn ProviderImage>) -> Result<(), LinkError> {
        let name = image.soname();
        if name.is_empty() || name.contains(['/', '\\', '\0']) {
            return Err(LinkError::InvalidProvider("invalid provider SONAME".into()));
        }
        if self.images.contains_key(name) {
            return Err(LinkError::InvalidProvider(format!(
                "duplicate provider {name}"
            )));
        }
        // A registered image owns and pins this mapping. Resolve its identity
        // once; dependency lookup must not reopen every registered DLL.
        let path = image
            .canonical_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                std::fs::canonicalize(image.path()).unwrap_or_else(|_| image.path().to_path_buf())
            });
        if self.paths.contains_key(&path) {
            return Err(LinkError::InvalidProvider(
                "one facade cannot own multiple SONAMEs".into(),
            ));
        }
        validate_symbols(image.as_ref())?;
        self.paths.insert(path, name.to_owned());
        self.images
            .insert(name.to_owned(), ProviderEntry::Ready(image));
        Ok(())
    }

    pub fn contains(&self, soname: &str) -> bool {
        self.images.contains_key(soname)
    }

    /// Only the registered interpreter can act as a native guest command.
    /// Compare immutable bytes so aliases and copied rootfs images work without
    /// accepting arbitrary PE files or trusting the executable's basename.
    pub fn is_interpreter_image(&self, bytes: &[u8]) -> bool {
        bytes.starts_with(b"MZ")
            && self
                .interpreter_image
                .get_or_init(|| {
                    let entry = self.images.get("ld-linux-x86-64.so.2")?;
                    crate::snapshot_guest_image(entry.path()).ok()
                })
                .as_ref()
                .is_some_and(|image| image.as_slice() == bytes)
    }

    /// Resolve identity without opening an image, in particular for RTLD_NOLOAD.
    pub(crate) fn registered_path(&self, path: &Path, soname: &str) -> Option<std::path::PathBuf> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let name = self
            .paths
            .get(&canonical)
            .map(String::as_str)
            .unwrap_or(soname);
        self.images
            .get(name)
            .map(|entry| entry.path().to_path_buf())
    }

    /// Reserve an exact SONAME and canonical file identity without invoking
    /// its factory. The owner must pin the inspected file until the factory
    /// is released. Loading errors are propagated by `get`, never hidden as
    /// an absent dependency or silently retried through ELF search paths.
    pub fn register_deferred(
        &mut self,
        name: String,
        path: std::path::PathBuf,
        factory: impl FnOnce() -> Result<Arc<dyn ProviderImage>, LinkError> + Send + 'static,
    ) -> Result<(), LinkError> {
        if name.is_empty() || name.contains(['/', '\\', '\0']) || self.images.contains_key(&name) {
            return Err(LinkError::InvalidProvider(
                "invalid or duplicate provider SONAME".into(),
            ));
        }
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if self.paths.contains_key(&path) {
            return Err(LinkError::InvalidProvider(
                "one facade cannot own multiple SONAMEs".into(),
            ));
        }
        self.paths.insert(path.clone(), name.clone());
        self.images.insert(
            name,
            ProviderEntry::Deferred {
                factory: Mutex::new(Some(Box::new(factory))),
                image: OnceLock::new(),
                path,
            },
        );
        Ok(())
    }

    pub fn get(&self, soname: &str) -> Result<Option<Arc<dyn ProviderImage>>, LinkError> {
        self.images
            .get(soname)
            .map(|entry| entry.get(soname))
            .transpose()
    }

    pub(crate) fn by_path(&self, path: &Path) -> Result<Option<Arc<dyn ProviderImage>>, LinkError> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        match self.paths.get(&canonical) {
            Some(name) => self.get(name),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[path = "provider_deferred_tests.rs"]
mod deferred_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{DllProvider, Provider, Scope};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Image {
        name: String,
        path: PathBuf,
        canonical_path: Option<PathBuf>,
        symbols: Vec<ProviderSymbol>,
        copied: Arc<AtomicUsize>,
    }

    impl ProviderImage for Image {
        fn soname(&self) -> &str {
            &self.name
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn canonical_path(&self) -> Option<&Path> {
            self.canonical_path.as_deref()
        }
        fn base(&self) -> usize {
            0x1000
        }
        fn symbols(&self) -> &[ProviderSymbol] {
            &self.symbols
        }
        fn redirect_copy(&self, _: &str, address: usize, size: u64) -> Result<(), LinkError> {
            if size != 4 {
                return Err(LinkError::InvalidProvider(
                    "incompatible copied object size".into(),
                ));
            }
            self.copied.store(address, Ordering::SeqCst);
            Ok(())
        }
    }

    fn image() -> Image {
        Image {
            name: "libtest.so.1".into(),
            path: PathBuf::from("libtest.so.1"),
            canonical_path: None,
            symbols: vec![ProviderSymbol {
                name: "counter".into(),
                address: 0x1230,
                kind: ProviderSymbolKind::Object,
                size: 4,
                versions: vec!["TEST_1.0".into(), "TEST_2.0".into()],
                default_version: Some("TEST_2.0".into()),
            }],
            copied: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[test]
    fn provider_resolution_preserves_data_size_and_exact_versions() {
        let image = Arc::new(image());
        let mut registry = ProviderRegistry::new();
        registry.register(image).unwrap();
        let mut scope = Scope::default();
        scope.dlls.push(DllProvider::from_registered(
            registry.get("libtest.so.1").unwrap().unwrap(),
        ));
        scope.push_provider(Provider::Dll(0));
        for version in [None, Some("TEST_1.0"), Some("TEST_2.0")] {
            let result = scope.resolve(&[], "counter", version).unwrap().unwrap();
            assert_eq!(result.address, Some(0x1230));
            assert_eq!(result.size, 4);
        }
        assert!(
            scope
                .resolve(&[], "counter", Some("GLIBC_2.2.5"))
                .unwrap()
                .is_none()
        );
        assert!(
            scope
                .resolve(&[], "kinakaze_abi_counter", None)
                .unwrap()
                .is_none()
        );
        scope.set_global(Provider::Dll(0), false);
        assert!(scope.resolve(&[], "counter", None).unwrap().is_none());
    }

    #[test]
    fn bare_export_never_claims_every_glibc_version() {
        let mut image = image();
        image.symbols[0].versions.clear();
        image.symbols[0].default_version = None;
        let provider = DllProvider::from_registered(Arc::new(image));
        assert!(provider.resolve("counter", None).unwrap().is_some());
        assert!(
            provider
                .resolve("counter", Some("GLIBC_2.2.5"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn version_only_definition_requires_explicit_lookup() {
        let mut image = image();
        image.symbols[0].default_version = None;
        let provider = DllProvider::from_registered(Arc::new(image));
        assert!(provider.resolve("counter", None).unwrap().is_none());
        assert!(
            provider
                .resolve("counter", Some("TEST_1.0"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn registry_rejects_ambiguous_metadata_and_aliases() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(image())).unwrap();
        assert!(registry.register(Arc::new(image())).is_err());
        let mut alias = image();
        alias.name = "libalias.so.1".into();
        assert!(registry.register(Arc::new(alias)).is_err());
        for case in 0..4 {
            let mut bad = image();
            match case {
                0 => bad.symbols[0].size = 0,
                1 => bad.symbols[0].address = 0,
                2 => bad.symbols[0].default_version = Some("UNKNOWN".into()),
                _ => bad.symbols.push(bad.symbols[0].clone()),
            }
            assert!(ProviderRegistry::new().register(Arc::new(bad)).is_err());
        }
    }

    #[test]
    fn registry_path_index_preserves_canonical_identity() {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-provider-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(root.join("child")).unwrap();
        let path = root.join("libtest.so.1");
        std::fs::write(&path, b"provider identity fixture").unwrap();
        let alternate = root.join("child/../libtest.so.1");
        for cached in [false, true] {
            let mut original = image();
            original.path = path.clone();
            original.canonical_path = cached.then(|| path.canonicalize().unwrap());
            let mut registry = ProviderRegistry::new();
            registry.register(Arc::new(original)).unwrap();
            assert!(Arc::ptr_eq(
                &registry.get("libtest.so.1").unwrap().unwrap(),
                &registry.by_path(&alternate).unwrap().unwrap(),
            ));
            let mut alias = image();
            alias.name = "libalias.so.1".into();
            alias.path = alternate.clone();
            alias.canonical_path = cached.then(|| alternate.canonicalize().unwrap());
            assert!(registry.register(Arc::new(alias)).is_err());
            assert!(
                registry
                    .by_path(&root.join("missing.so"))
                    .unwrap()
                    .is_none()
            );
            assert!(
                registry
                    .by_path(&root.join("child/libtest.so.1"))
                    .unwrap()
                    .is_none()
            );
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root.join("child")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn invalid_provider_does_not_reserve_a_path() {
        let mut registry = ProviderRegistry::new();
        let mut bad = image();
        bad.symbols[0].address = 0;
        assert!(registry.register(Arc::new(bad)).is_err());
        assert!(
            registry
                .by_path(Path::new("libtest.so.1"))
                .unwrap()
                .is_none()
        );
        registry.register(Arc::new(image())).unwrap();
        assert!(
            registry
                .by_path(Path::new("libtest.so.1"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn copy_redirect_targets_only_the_resolved_provider_definition() {
        let image = Arc::new(image());
        let copied = image.copied.clone();
        let provider = DllProvider::from_registered(image);
        provider
            .redirect_copy("counter", 0x2340, 4, 0x9990)
            .unwrap();
        assert_eq!(copied.load(Ordering::SeqCst), 0);
        provider
            .redirect_copy("counter", 0x2340, 4, 0x1230)
            .unwrap();
        assert_eq!(copied.load(Ordering::SeqCst), 0x2340);
        assert!(
            provider
                .redirect_copy("counter", 0x3450, 8, 0x1230)
                .is_err()
        );
    }

    #[test]
    fn provider_addresses_stay_pinned_and_reopen_without_rebinding() {
        let image = Arc::new(image());
        let weak = Arc::downgrade(&image);
        let mut provider = DllProvider::from_registered(image);
        provider.release_reference();
        assert!(!provider.is_loaded());
        assert!(provider.resolve("counter", None).unwrap().is_none());
        assert!(weak.upgrade().is_some());
        provider.add_reference().unwrap();
        assert_eq!(
            provider.resolve("counter", None).unwrap().unwrap().address,
            Some(0x1230)
        );
        drop(provider);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn dlopen_uses_bound_facade_and_dlvsym_honors_versions_and_visibility() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(image())).unwrap();
        let mut linker = crate::Linker::new(crate::SearchPaths::default());
        linker
            .register_provider_registry(Arc::new(registry))
            .unwrap();
        // No file exists at this SONAME. A bound provider must be selected before
        // filesystem probing or a second generic ELF mapping can happen.
        let handle = unsafe { linker.open_runtime(Some(Path::new("libtest.so.1")), 2) }.unwrap();
        assert_eq!(
            linker
                .lookup_runtime(Some(handle), "counter")
                .unwrap()
                .unwrap()
                .size,
            4
        );
        assert!(linker.lookup("counter").unwrap().is_none()); // RTLD_LOCAL
        assert!(
            linker
                .lookup_runtime_version(Some(handle), "counter", "TEST_1.0")
                .unwrap()
                .is_some()
        );
        assert!(
            linker
                .lookup_runtime_version(Some(handle), "counter", "GLIBC_2.2.5")
                .unwrap()
                .is_none()
        );
        let global =
            unsafe { linker.open_runtime(Some(Path::new("libtest.so.1")), 2 | 4 | 0x100) }.unwrap();
        assert!(
            linker
                .lookup_runtime_version(None, "counter", "TEST_2.0")
                .unwrap()
                .is_some()
        );
        linker.close_runtime(handle).unwrap();
        linker.close_runtime(global).unwrap();
        assert!(linker.lookup("counter").unwrap().is_none());
        assert!(unsafe { linker.open_runtime(Some(Path::new("libtest.so.1")), 2 | 4) }.is_err());
        assert!(
            linker
                .register_provider_registry(Arc::new(ProviderRegistry::new()))
                .is_err()
        );
    }
}
