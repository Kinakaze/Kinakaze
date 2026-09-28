use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "kinakaze-catalog-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.0.join(name), bytes).unwrap();
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// Structurally valid for read-only PE inspection, deliberately not an
// executable Windows DLL. Discovery must never ask Windows to load it.
fn fixture(symbols: &[(&str, bool)]) -> Vec<u8> {
    let mut bytes = vec![0; 0x1400];
    let put16 = |bytes: &mut [u8], at: usize, value: u16| {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    };
    let put32 = |bytes: &mut [u8], at: usize, value: u32| {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    };
    bytes[..2].copy_from_slice(b"MZ");
    put32(&mut bytes, 0x3c, 0x80);
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    put16(&mut bytes, 0x84, 0x8664);
    put16(&mut bytes, 0x86, 2);
    put16(&mut bytes, 0x94, 240);
    put16(&mut bytes, 0x96, 0x2000);
    put16(&mut bytes, 0x98, 0x20b);
    put32(&mut bytes, 0x98 + 108, 1);
    put32(&mut bytes, 0x98 + 112, 0x1000);
    put32(&mut bytes, 0x98 + 116, 0x800);
    for (index, rva, raw, size, flags) in [
        (0, 0x1000, 0x200, 0x1000, 0x6000_0020),
        (1, 0x2000, 0x1200, 0x200, 0xc000_0040),
    ] {
        let section = 0x188 + index * 40;
        put32(&mut bytes, section + 8, size);
        put32(&mut bytes, section + 12, rva);
        put32(&mut bytes, section + 16, size);
        put32(&mut bytes, section + 20, raw);
        put32(&mut bytes, section + 36, flags);
    }
    let mut symbols = symbols.to_vec();
    symbols.sort_by_key(|symbol| symbol.0);
    for (offset, value) in [
        (12, 0x1200),
        (20, symbols.len() as u32),
        (24, symbols.len() as u32),
        (28, 0x1040),
        (32, 0x1080),
        (36, 0x10c0),
    ] {
        put32(&mut bytes, 0x200 + offset, value);
    }
    bytes[0x400..0x40d].copy_from_slice(b"libfixture.so");
    let mut at = 0x480;
    for (index, (name, object)) in symbols.into_iter().enumerate() {
        put32(
            &mut bytes,
            0x240 + index * 4,
            if object { 0x2010 } else { 0x1900 },
        );
        put32(&mut bytes, 0x280 + index * 4, (at + 0xe00) as u32);
        put16(&mut bytes, 0x2c0 + index * 2, index as u16);
        bytes[at..at + name.len()].copy_from_slice(name.as_bytes());
        at += name.len() + 1;
    }
    bytes
}

fn runtime() -> Vec<u8> {
    fixture(&[
        ("kinakaze_runtime_open_v1", false),
        ("kinakaze_runtime_abi_version", false),
    ])
}

#[test]
fn catalog_defers_layout_loading_and_pins_pending_files() {
    let directory = Directory::new();
    directory.write("libruntime.so", &runtime());
    directory.write(
        "liblate.so",
        &fixture(&[("kinakaze_module_object_v1", false), ("object", true)]),
    );
    directory.write("libguest.so", b"\x7fELF");
    let catalog = ModuleCatalog::discover(&directory.0).unwrap();
    assert_eq!(catalog.modules.len(), 2);
    let runtime = catalog
        .modules
        .iter()
        .find(|module| module.id == 0)
        .unwrap();
    assert_eq!(runtime.soname, "libruntime.so");
    assert_eq!(runtime.native_name, "libfixture.so");
    let (metadata, owner) = runtime.materialize().unwrap();
    assert!(owner.is_none());
    assert_eq!(metadata.exports[0].name, "kinakaze_runtime_abi_version");
    let late = catalog
        .modules
        .iter()
        .find(|module| module.soname == "liblate.so")
        .unwrap();
    assert_eq!(late.native_name, "libfixture.so");
    // Invalid executable headers are encountered only when its object layout
    // is needed; all read-only export validation already passed at discovery.
    assert!(late.materialize().is_err());
    let retained = catalog.retain_images();
    drop(catalog);
    let path = directory.0.join("liblate.so");
    assert!(fs::OpenOptions::new().write(true).open(&path).is_err());
    assert!(fs::remove_file(&path).is_err());
    drop(retained);
    assert!(fs::OpenOptions::new().write(true).open(&path).is_ok());
}

#[test]
fn catalog_checks_unused_rust_exports_and_runtime_identity_up_front() {
    let directory = Directory::new();
    directory.write("libruntime.so", &runtime());
    let mut malformed = fixture(&[("_Rinternal", false), ("kinakaze_module_object_v1", false)]);
    malformed[0x480] = 0xff;
    directory.write("libunused.so", &malformed);
    assert!(ModuleCatalog::discover(&directory.0).is_err());
    fs::remove_file(directory.0.join("libunused.so")).unwrap();
    directory.write("libduplicate.so", &runtime());
    assert!(ModuleCatalog::discover(&directory.0).is_err());
    fs::remove_file(directory.0.join("libduplicate.so")).unwrap();
    directory.write(
        "libruntime.so",
        &fixture(&[
            ("kinakaze_runtime_open_v1", false),
            ("kinakaze_provider_initialize_v1", false),
        ]),
    );
    assert!(ModuleCatalog::discover(&directory.0).is_err());
}

#[test]
fn demanded_metadata_validates_names_and_versions_and_eager_api_still_rejects_it() {
    let directory = Directory::new();
    directory.write("libruntime.so", &runtime());
    for symbol in ["bad-name", "valid@bad-version", "valid@"] {
        directory.write(
            "liblate.so",
            &fixture(&[("kinakaze_module_object_v1", false), (symbol, false)]),
        );
        let catalog = ModuleCatalog::discover(&directory.0).unwrap();
        let late = catalog
            .modules
            .iter()
            .find(|module| module.soname == "liblate.so")
            .unwrap();
        assert!(late.materialize().is_err(), "{symbol}");
        assert!(ModuleSet::discover(&directory.0).is_err(), "{symbol}");
    }
}
