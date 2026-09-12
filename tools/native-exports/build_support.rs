use std::path::PathBuf;

pub fn emit_exports() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let directory = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("crate directory"));
    let definition = directory.join("exports.def");
    println!("cargo:rerun-if-changed={}", definition.display());
    println!("cargo:rustc-link-arg=/DEF:{}", definition.display());
    // Cargo names its build artifact .dll; LIBRARY defines the native import
    // name. Packaging only copies those unchanged bytes to that .so filename.
    println!("cargo:rustc-link-arg=/IGNORE:4070");
}
