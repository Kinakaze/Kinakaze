include!("../../tools/native-exports/build_support.rs");

fn main() {
    emit_exports();
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
        return;
    }
    for file in ["bridge.c", "libm.h", "powl.c"] {
        println!("cargo:rerun-if-changed=src/ld80/{file}");
    }
    println!("cargo:rerun-if-env-changed=KINAKAZE_CLANG");
    let compiler = std::env::var_os("KINAKAZE_CLANG").unwrap_or_else(|| "clang".into());
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("powl80.obj");
    let target = std::env::var("TARGET").unwrap();
    let status = std::process::Command::new(&compiler)
        .arg(format!("--target={target}"))
        .args([
            "-c",
            "-O2",
            "-mlong-double-80",
            "-ffp-model=strict",
            "-ffreestanding",
            "-fno-builtin",
            "-Wno-constant-conversion",
            "src/ld80/bridge.c",
            "-o",
        ])
        .arg(&output)
        .status()
        .expect("Clang is required for the IEEE binary80 math implementation (KINAKAZE_CLANG)");
    assert!(
        status.success(),
        "could not compile the IEEE binary80 power implementation"
    );
    println!("cargo:rustc-link-arg={}", output.display());
}
