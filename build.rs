// SPDX-FileCopyrightText: 2026 Toyota Connected North America
// SPDX-License-Identifier: Apache-2.0

// Generates Rust bindings for the installed ivi-homescreen shared ABI
// (ihs/*.h) and links libihs_shared.so.1 through ivi-homescreen-shared.pc.
// The build is kept out of the shell's CMake on purpose, so it cross-compiles
// like any cargo crate. Set PKG_CONFIG_PATH / PKG_CONFIG_SYSROOT_DIR as for
// any pkg-config consumer; a Yocto build does this through the cargo bbclass.

use std::env;
use std::path::{Path, PathBuf};

/// The platform-view ABI the module is written against: image layers (1.16),
/// render_device (1.14), scanout_hint (1.13). The .pc version does not track
/// it, so it is read from the headers.
const ABI_MINOR: u32 = 16;

/// IHS_SHARED_ABI_<field> from ihs/ihs_version.h under one of @p includes.
fn abi(includes: &[PathBuf], field: &str) -> Option<(PathBuf, u32)> {
    let name = format!("#define IHS_SHARED_ABI_{field} ");
    includes.iter().find_map(|dir| {
        let path = Path::new(dir).join("ihs/ihs_version.h");
        let text = std::fs::read_to_string(&path).ok()?;
        let value = text.lines().find_map(|l| l.strip_prefix(&name))?;
        let value = value.trim().trim_end_matches('u').parse().ok()?;
        Some((path, value))
    })
}

fn main() {
    let lib = pkg_config::Config::new()
        .atleast_version("1.0.0")
        .probe("ivi-homescreen-shared")
        .expect("ivi-homescreen-shared.pc not found; set PKG_CONFIG_PATH");

    match (
        abi(&lib.include_paths, "MAJOR"),
        abi(&lib.include_paths, "MINOR"),
    ) {
        (Some((path, 1)), Some((_, minor))) => {
            println!("cargo:rerun-if-changed={}", path.display());
            assert!(
                minor >= ABI_MINOR,
                "ivi-homescreen-shared at {} has platform-view ABI 1.{minor}; \
                 ihs_wl_server needs 1.{ABI_MINOR} or newer",
                path.display()
            );
        }
        (Some((path, major)), _) => panic!(
            "ivi-homescreen-shared at {} has platform-view ABI major {major}; \
             ihs_wl_server is written against 1.x",
            path.display()
        ),
        _ => panic!("no ihs/ihs_version.h under {:?}", lib.include_paths),
    }

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
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ihs_bindings.rs");
    builder
        .generate()
        .expect("bindgen over ihs/*.h failed")
        .write_to_file(out)
        .unwrap();
}
