//! Links the archives `cargo xtask gpu nvk` or `anv` builds, by the feature.
use std::{env, path::PathBuf};

fn main() {
    let anv = env::var_os("CARGO_FEATURE_ANV").is_some();
    let driver = if anv { "anv" } else { "nvk" };
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let directory = manifest.join("../../target/gpu").join(driver);
    // LLVM and libc++ are every driver's, where `llvm::build` puts them.
    let llvm = manifest.join("../../target/gpu/nvk");
    // Mesa links its Vulkan runtime and drivers whole: their entry points
    // meet in tables by weak reference, which pulls no member from an archive.
    let lazy = format!("stela-{driver}");
    for (name, kind, at) in [
        ("stela-vulkan", "static:+whole-archive", &directory),
        (lazy.as_str(), "static", &directory),
        ("stela-llvm", "static", &llvm),
    ] {
        let archive = at.join(format!("lib{name}.a"));
        assert!(
            archive.is_file(),
            "{} is missing: run `cargo xtask gpu {driver}`",
            archive.display()
        );
        println!("cargo:rustc-link-search=native={}", at.display());
        println!("cargo:rerun-if-changed={}", archive.display());
        println!("cargo:rustc-link-lib={kind}={name}");
    }
    // What turns NIR into machine code: the C archives (the driver, NIR and
    // the Vulkan runtime), NVK's Rust NAK and NIL, and the toolchain. The
    // driver's own pipeline cache UUID is the same for every build
    // (`compat/mesa/build_id.c`), so this keys the pipeline store instead.
    let sources: &[&str] = if anv {
        &[
            ".",
            "../../target/gpu/anv/libstela-anv.a",
            "../../target/gpu/anv/libstela-vulkan.a",
            "../../third_party/mesa/src/intel/compiler",
            "../../rust-toolchain.toml",
        ]
    } else {
        &[
            ".",
            "../../target/gpu/nvk/libstela-nvk.a",
            "../../target/gpu/nvk/libstela-vulkan.a",
            "../../target/gpu/nvk/build/src/compiler/rust/libcompiler.rlib.p",
            "../../target/gpu/nvk/build/src/nouveau/compiler/latencies",
            "../../target/gpu/nvk/build/src/nouveau/compiler/nak_bindings.rs",
            "../../target/gpu/nvk/build/src/nouveau/headers/lib.rs",
            "../../target/gpu/nvk/build/src/nouveau/nil/nil_bindings.rs",
            "../../third_party/mesa/src/compiler/rust",
            "../../third_party/mesa/src/nouveau/compiler",
            "../../third_party/mesa/src/nouveau/headers/nv_push_rs",
            "../../third_party/mesa/src/nouveau/nil",
            "../../third_party/mesa/src/nouveau/rust",
            "../../rust-toolchain.toml",
        ]
    };
    stela_fingerprint::emit("STELA_MESA_COMPILER_SHA256", sources);
}
