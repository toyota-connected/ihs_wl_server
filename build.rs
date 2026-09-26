// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Generates Rust bindings for the installed ivi-homescreen shared ABI
// (ihs/*.h) and links libihs_shared.so.1 through ivi-homescreen-shared.pc.
// The build is kept out of the shell's CMake on purpose, so it cross-compiles
// like any cargo crate. Set PKG_CONFIG_PATH / PKG_CONFIG_SYSROOT_DIR as for
// any pkg-config consumer; a Yocto build does this through the cargo bbclass.

use std::env;
use std::path::PathBuf;

/// The platform-view ABI the module is written against: image layers (1.16),
/// render_device (1.14), scanout_hint (1.13). The .pc version does not track
/// it, so it is read from the headers bindgen parsed -- the ones clang
/// resolved, wherever they are.
const ABI_MINOR: u32 = 16;

/// `pub const IHS_SHARED_ABI_<field>: u32 = N;` in the generated bindings.
fn abi(bindings: &str, field: &str) -> Option<u32> {
    let name = format!("pub const IHS_SHARED_ABI_{field}: u32 = ");
    bindings
        .lines()
        .find_map(|l| l.trim().strip_prefix(&name))
        .and_then(|v| v.trim_end_matches(';').parse().ok())
}

fn main() {
    let lib = pkg_config::Config::new()
        .atleast_version("1.0.0")
        .probe("ivi-homescreen-shared")
        .expect("ivi-homescreen-shared.pc not found; set PKG_CONFIG_PATH");

    // A native dev build against a local shared/ install gets an rpath so
    // `cargo test` runs without LD_LIBRARY_PATH. Never for a cross or
    // sysroot build: that would bake a build-host path into the target.
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");
    let native = env::var("TARGET").ok() == env::var("HOST").ok()
        && env::var_os("PKG_CONFIG_SYSROOT_DIR").is_none();
    if native {
        for dir in &lib.link_paths {
            if !dir.starts_with("/usr") && !dir.starts_with("/lib") {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
            }
        }
    }

    let mut builder = bindgen::Builder::default()
        .header("src/ffi/ihs_wrapper.h")
        .clang_arg("-std=c11")
        .allowlist_function("ihs_.*")
        .allowlist_type("Ihs.*")
        .allowlist_var("IHS_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for inc in &lib.include_paths {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }
    let bindings = builder.generate().expect("bindgen over ihs/*.h failed");

    // Before the bindings are used, so an old shell fails here with one line
    // rather than in rustc with a missing type per call site.
    let text = bindings.to_string();
    match (abi(&text, "MAJOR"), abi(&text, "MINOR")) {
        (Some(1), Some(minor)) => assert!(
            minor >= ABI_MINOR,
            "ivi-homescreen-shared has platform-view ABI 1.{minor}; \
             ihs_wl_server needs 1.{ABI_MINOR} or newer"
        ),
        (Some(major), _) => panic!(
            "ivi-homescreen-shared has platform-view ABI major {major}; \
             ihs_wl_server is written against 1.x"
        ),
        _ => panic!("no IHS_SHARED_ABI_MAJOR/MINOR in the ihs headers"),
    }

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ihs_bindings.rs");
    bindings.write_to_file(out).unwrap();
}
