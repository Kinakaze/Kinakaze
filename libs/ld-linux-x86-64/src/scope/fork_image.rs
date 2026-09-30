//! A fork's native images are already loaded at verified addresses. Retain
//! that mapping and the registry's immutable source pins immediately, while
//! reconstructing its export metadata only if a later loader operation needs it.
use crate::{LinkError, ProviderImage, ProviderRegistry};
use std::{os::windows::ffi::OsStrExt, path::Path, sync::Arc};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::GetModuleHandleExW,
};

struct Pin(HMODULE);
// Windows loader references are process-wide and remain valid on any thread.
unsafe impl Send for Pin {}
unsafe impl Sync for Pin {}
impl Drop for Pin {
    fn drop(&mut self) {
        // Exactly one owning GetModuleHandleExW reference.
        unsafe { FreeLibrary(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderSymbol, ProviderSymbolKind, scope::DllProvider};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Image {
        path: std::path::PathBuf,
        base: usize,
        symbols: Vec<ProviderSymbol>,
    }
    impl ProviderImage for Image {
        fn soname(&self) -> &str {
            "libfork.so"
        }
        fn path(&self) -> &Path {
            &self.path
        }
        fn canonical_path(&self) -> Option<&Path> {
            Some(&self.path)
        }
        fn base(&self) -> usize {
            self.base
        }
        fn symbols(&self) -> &[ProviderSymbol] {
            &self.symbols
        }
        fn redirect_copy(&self, _: &str, _: usize, _: u64) -> Result<(), LinkError> {
            Ok(())
        }
    }
    fn registry(fail: bool) -> (Arc<ProviderRegistry>, Arc<AtomicUsize>, usize) {
        let path = std::env::current_exe().unwrap().canonicalize().unwrap();
        let base = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null())
        } as usize;
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let image = Image {
            path: path.clone(),
            base,
            symbols: vec![ProviderSymbol {
                name: "answer".into(),
                address: base + 512,
                kind: ProviderSymbolKind::Function,
                size: 0,
                versions: vec!["VERSION_1".into()],
                default_version: Some("VERSION_1".into()),
            }],
        };
        let mut registry = ProviderRegistry::new();
        registry
            .register_deferred("libfork.so".into(), path, move || {
                counter.fetch_add(1, Ordering::SeqCst);
                if fail {
                    return Err(LinkError::InvalidProvider(
                        "fixture metadata failure".into(),
                    ));
                }
                Ok(Arc::new(image))
            })
            .unwrap();
        (Arc::new(registry), count, base)
    }

    #[test]
    fn restored_image_is_pinned_before_metadata_is_demanded() {
        let (registry, count, base) = registry(false);
        let mut provider = DllProvider::from_fork(registry, "libfork.so", base).unwrap();
        assert_eq!(provider.base(), base);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        provider.release_reference();
        assert!(provider.resolve("answer", None).unwrap().is_none());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        provider.add_reference().unwrap();
        assert_eq!(provider.symbol("answer").unwrap(), Some(base + 512));
        assert!(
            provider
                .resolve("answer", Some("VERSION_2"))
                .unwrap()
                .is_none()
        );
        assert_eq!(provider.symbol("answer").unwrap(), Some(base + 512));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn restored_image_rejects_wrong_base_and_retains_metadata_errors() {
        let (registry, count, base) = registry(true);
        assert!(DllProvider::from_fork(Arc::clone(&registry), "libfork.so", base + 1).is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let provider = DllProvider::from_fork(registry, "libfork.so", base).unwrap();
        assert!(provider.symbol("answer").is_err());
        assert!(provider.symbol("answer").is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}

pub(super) struct Pending {
    registry: Arc<ProviderRegistry>,
    path: std::path::PathBuf,
    _pin: Pin,
}

impl Pending {
    pub(super) fn new(
        registry: &Arc<ProviderRegistry>,
        name: &str,
        base: usize,
    ) -> Result<Option<Self>, LinkError> {
        static EAGER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *EAGER.get_or_init(|| {
            std::env::var_os("KINAKAZE_FORK_PROVIDER_METADATA")
                .is_some_and(|value| value == "eager")
        }) {
            return Ok(None);
        }
        let path = registry
            .image_path(name)
            .ok_or_else(|| LinkError::InvalidProvider(format!("missing fork provider {name}")))?;
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if !path.is_absolute() || wide.contains(&0) {
            return Ok(None);
        }
        wide.push(0);
        let mut module = std::ptr::null_mut();
        // Full registered path, not a basename or a serialized callback. This
        // cannot load code or run DLL initializers. Success pins the live image.
        if unsafe { GetModuleHandleExW(0, wide.as_ptr(), &mut module) } == 0 {
            return Ok(None);
        }
        let pin = Pin(module);
        if module as usize != base {
            return Err(LinkError::InvalidProvider(format!(
                "fork provider moved: {name}"
            )));
        }
        Ok(Some(Self {
            registry: Arc::clone(registry),
            path: path.to_path_buf(),
            _pin: pin,
        }))
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn materialize(
        &self,
        name: &str,
        base: usize,
    ) -> Result<Arc<dyn ProviderImage>, LinkError> {
        let image = self
            .registry
            .get(name)?
            .ok_or_else(|| LinkError::InvalidProvider(format!("missing fork provider {name}")))?;
        if image.base() != base {
            return Err(LinkError::InvalidProvider(format!(
                "fork provider moved: {name}"
            )));
        }
        Ok(image)
    }
}
