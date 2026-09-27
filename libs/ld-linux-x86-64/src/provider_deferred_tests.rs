use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Image {
    name: &'static str,
    path: PathBuf,
    symbols: Vec<ProviderSymbol>,
    deferred: bool,
    lookups: Arc<AtomicUsize>,
}

impl ProviderImage for Image {
    fn soname(&self) -> &str {
        self.name
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
    fn has_deferred_addresses(&self) -> bool {
        self.deferred
    }
    fn symbol_address(&self, index: usize) -> Result<usize, LinkError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Ok(if self.deferred {
            0x1230 + index * 16
        } else {
            self.symbols[index].address
        })
    }
    fn redirect_copy(&self, _: &str, _: usize, _: u64) -> Result<(), LinkError> {
        Ok(())
    }
}

fn image() -> Image {
    Image {
        name: "libdeferred.so.1",
        path: PathBuf::from("libdeferred.so.1"),
        symbols: vec![ProviderSymbol {
            name: "value".into(),
            address: 0x1230,
            size: 4,
            kind: ProviderSymbolKind::Object,
            versions: vec!["TEST_1".into()],
            default_version: Some("TEST_1".into()),
        }],
        deferred: false,
        lookups: Arc::new(AtomicUsize::new(0)),
    }
}

#[test]
fn deferred_factory_runs_once_on_demand_even_with_concurrent_lookups() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let mut registry = ProviderRegistry::new();
    registry
        .register_deferred(
            "libdeferred.so.1".into(),
            PathBuf::from("libdeferred.so.1"),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5));
                Ok(Arc::new(image()))
            },
        )
        .unwrap();
    assert!(registry.contains("libdeferred.so.1"));
    assert!(registry.get("missing.so").unwrap().is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let registry = Arc::new(registry);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let registry = Arc::clone(&registry);
            std::thread::spawn(move || registry.get("libdeferred.so.1").unwrap().unwrap())
        })
        .collect();
    let images: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(images.iter().all(|image| Arc::ptr_eq(image, &images[0])));
    assert!(Arc::ptr_eq(
        &registry
            .by_path(Path::new("libdeferred.so.1"))
            .unwrap()
            .unwrap(),
        &images[0]
    ));
}

#[test]
fn failures_are_cached_and_reach_dlopen_without_falling_back_to_filesystem() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let mut registry = ProviderRegistry::new();
    registry
        .register_deferred(
            "libdeferred.so.1".into(),
            PathBuf::from("libdeferred.so.1"),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                Err(LinkError::InvalidProvider(
                    "intentional native failure".into(),
                ))
            },
        )
        .unwrap();
    for _ in 0..2 {
        assert!(
            registry
                .get("libdeferred.so.1")
                .err()
                .unwrap()
                .to_string()
                .contains("intentional native failure")
        );
    }
    let mut linker = crate::Linker::new(crate::SearchPaths::default());
    linker
        .register_provider_registry(Arc::new(registry))
        .unwrap();
    let error = unsafe { linker.open_runtime(Some(Path::new("libdeferred.so.1")), 2) }.unwrap_err();
    assert!(error.to_string().contains("intentional native failure"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn deferred_identity_and_symbol_validation_remain_mandatory() {
    for variant in 0..4 {
        let mut registry = ProviderRegistry::new();
        registry
            .register_deferred(
                "libdeferred.so.1".into(),
                PathBuf::from("libdeferred.so.1"),
                move || {
                    let mut value = image();
                    match variant {
                        0 => value.name = "wrong.so",
                        1 => value.path = PathBuf::from("wrong.so"),
                        2 => value.symbols[0].address = 0,
                        _ => value.symbols[0].default_version = Some("UNKNOWN".into()),
                    }
                    Ok(Arc::new(value))
                },
            )
            .unwrap();
        assert!(
            registry.get("libdeferred.so.1").is_err(),
            "variant={variant}"
        );
    }
}

#[test]
fn deferred_registration_rejects_aliases_without_loading_them() {
    let mut registry = ProviderRegistry::new();
    registry
        .register_deferred(
            "libdeferred.so.1".into(),
            PathBuf::from("libdeferred.so.1"),
            || panic!("must stay deferred"),
        )
        .unwrap();
    assert!(
        registry
            .register_deferred(
                "alias.so".into(),
                PathBuf::from("libdeferred.so.1"),
                || panic!("must not run")
            )
            .is_err()
    );
    assert!(
        registry
            .register_deferred(
                "libdeferred.so.1".into(),
                PathBuf::from("other.so"),
                || panic!("must not run")
            )
            .is_err()
    );
}

#[test]
fn symbols_resolve_only_after_matching_the_requested_name_and_version() {
    let mut value = image();
    value.deferred = true;
    value.symbols[0].address = 0;
    let calls = Arc::clone(&value.lookups);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(value)).unwrap();
    let provider = crate::scope::DllProvider::from_registered(
        registry.get("libdeferred.so.1").unwrap().unwrap(),
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(provider.resolve("missing", None).unwrap().is_none());
    assert!(
        provider
            .resolve("value", Some("UNKNOWN"))
            .unwrap()
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        provider
            .resolve("value", Some("TEST_1"))
            .unwrap()
            .unwrap()
            .address,
        Some(0x1230)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    provider.redirect_copy("value", 0x2000, 4, 0x1230).unwrap();
}

#[test]
fn noload_does_not_materialize_a_deferred_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let mut registry = ProviderRegistry::new();
    registry
        .register_deferred(
            "libdeferred.so.1".into(),
            PathBuf::from("libdeferred.so.1"),
            move || {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(image()))
            },
        )
        .unwrap();
    let mut linker = crate::Linker::new(crate::SearchPaths::default());
    linker
        .register_provider_registry(Arc::new(registry))
        .unwrap();
    assert!(unsafe { linker.open_runtime(Some(Path::new("libdeferred.so.1")), 2 | 4) }.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let handle = unsafe { linker.open_runtime(Some(Path::new("libdeferred.so.1")), 2) }.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        linker
            .lookup_runtime(Some(handle), "value")
            .unwrap()
            .unwrap()
            .address,
        Some(0x1230)
    );
    let resident =
        unsafe { linker.open_runtime(Some(Path::new("libdeferred.so.1")), 2 | 4) }.unwrap();
    linker.close_runtime(resident).unwrap();
    linker.close_runtime(handle).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
