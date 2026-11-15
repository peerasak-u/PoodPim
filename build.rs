// The prebuilt ONNX Runtime static library uses `@available` checks, which need
// `___isPlatformVersionAtLeast` from clang's compiler-rt. Rust's linker invocation
// doesn't add it, so link libclang_rt.osx.a from the active Xcode toolchain.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::process::Command::new("xcrun").args(["clang", "--print-resource-dir"]).output();
    if let Ok(out) = out {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("cargo:rustc-link-search=native={dir}/lib/darwin");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}
