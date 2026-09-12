//! Explicit, validated provider images. Ownership pins the native PE module
//! and, where still present, its ELF facade for every published guest address.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::LinkError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderSymbolKind {
    Function,
    Object,
}

#[derive(Clone, Debug)]
pub struct ProviderSymbol {
    pub name: String,
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
    fn base(&self) -> usize;
    fn symbols(&self) -> &[ProviderSymbol];
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
    images: HashMap<String, Arc<dyn ProviderImage>>,
    paths: HashMap<std::path::PathBuf, String>,
    pub(crate) restore_source: Option<std::path::PathBuf>,
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
        let path =
            std::fs::canonicalize(image.path()).unwrap_or_else(|_| image.path().to_path_buf());
        if self.paths.contains_key(&path) {
            return Err(LinkError::InvalidProvider(
                "one facade cannot own multiple SONAMEs".into(),
            ));
        }
        let mut names = HashSet::new();
        for symbol in image.symbols() {
            if symbol.name.is_empty() || symbol.name.contains('\0') || !names.insert(&symbol.name) {
                return Err(LinkError::InvalidProvider(format!(
                    "invalid or duplicate symbol in {name}"
                )));
            }
            if symbol.address == 0
                || (symbol.kind == ProviderSymbolKind::Object && symbol.size == 0)
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
        self.paths.insert(path, name.to_owned());
        self.images.insert(name.to_owned(), image);
        Ok(())
    }

    pub fn contains(&self, soname: &str) -> bool {
        self.images.contains_key(soname)
    }

    pub fn get(&self, soname: &str) -> Option<Arc<dyn ProviderImage>> {
        self.images.get(soname).cloned()
    }

    pub(crate) fn by_path(&self, path: &Path) -> Option<Arc<dyn ProviderImage>> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.paths.get(&canonical).and_then(|name| self.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{DllProvider, Provider, Scope};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Image {
        name: String,
        path: PathBuf,
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
            registry.get("libtest.so.1").unwrap(),
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
        let mut original = image();
        original.path = path.clone();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(original)).unwrap();
        let alternate = root.join("child/../libtest.so.1");
        assert!(Arc::ptr_eq(
            &registry.get("libtest.so.1").unwrap(),
            &registry.by_path(&alternate).unwrap(),
        ));
        let mut alias = image();
        alias.name = "libalias.so.1".into();
        alias.path = alternate;
        assert!(registry.register(Arc::new(alias)).is_err());
        assert!(registry.by_path(&root.join("missing.so")).is_none());
        assert!(registry.by_path(&root.join("child/libtest.so.1")).is_none());
        drop(registry);
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
        assert!(registry.by_path(Path::new("libtest.so.1")).is_none());
        registry.register(Arc::new(image())).unwrap();
        assert!(registry.by_path(Path::new("libtest.so.1")).is_some());
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
