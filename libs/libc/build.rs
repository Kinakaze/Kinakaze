include!("../../tools/native-exports/build_support.rs");

fn main() {
    emit_exports();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // raw-dylib cannot express SysV on Windows. These prefixed math imports
    // use an ordinary import library and leave host CRT calls untouched.
    println!("cargo:rerun-if-changed=math-imports.def");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let result = std::process::Command::new("lib.exe")
        .args(["/NOLOGO", "/MACHINE:X64", "/DEF:math-imports.def"])
        .arg(format!(
            "/OUT:{}",
            out.join("kinakaze_math_imports.lib").display()
        ))
        .output()
        .expect("MSVC lib.exe must be on PATH");
    assert!(
        result.status.success(),
        "math import library: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo:rustc-link-search=native={}", out.display());
}
