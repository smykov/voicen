//! Gives the integration-test exes of this crate the shell's C runtime (T-099 invariant 4,
//! F-001 class; docs/decisions/ci-toolchain.md): static vcruntime / libcmt / libcpmt with the
//! dynamic UCRT, the hybrid CRT tauri-build's `static_vcruntime` gives `voicen.exe`. whisper.cpp
//! is compiled `/MT` (.cargo/config.toml [env]); a Rust exe without `crt-static` links the
//! dynamic VC runtime, so without these arguments `transcribe_wav-*.exe` imports
//! VCRUNTIME140.dll and the CI `dumpbin` check (scripts/ci/crt-imports.sh) refuses it.
//!
//! The arguments are the ones tauri-build 2.7.1 `static_vcruntime::build` emits, scoped to
//! `rustc-link-arg-tests` (the F-002 precedent in src-tauri/build.rs). The empty `msvcrt.lib`
//! that replaces the one rustc names is found through `/LIBPATH` in those arguments, never
//! through `rustc-link-search`: a search path reaches every target of the invocation, doctests
//! of other crates included (F-001). std only: the crate has no build dependencies.

use std::io::Write;
use std::path::PathBuf;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    // Machine type of the empty library (tauri-build 2.7.1 `override_msvcrt_lib`).
    let machine: [u8; 2] = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => [0x64, 0x86],
        Ok("x86") => [0x4C, 0x01],
        // Another architecture: tauri-build does not override msvcrt.lib there either.
        _ => return,
    };
    // Cargo sets it for every build-script run; without it there is nowhere to put the stub.
    let Some(out_dir) = std::env::var_os("OUT_DIR") else {
        panic!("OUT_DIR is not set; cannot write the empty msvcrt.lib");
    };
    let out_dir = PathBuf::from(out_dir);
    let stub = out_dir.join("msvcrt.lib");
    let mut bytes = Vec::with_capacity(STUB_BODY.len() + 2);
    bytes.extend_from_slice(&machine);
    bytes.extend_from_slice(STUB_BODY);
    let written = std::fs::File::create(&stub).and_then(|mut f| f.write_all(&bytes));
    if let Err(e) = written {
        panic!("cannot write {}: {e}", stub.display());
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg-tests=/LIBPATH:{}", out_dir.display());
    for lib in [
        "libvcruntimed.lib",
        "vcruntime.lib",
        "vcruntimed.lib",
        "libcmtd.lib",
        "msvcrt.lib",
        "msvcrtd.lib",
        "libucrt.lib",
        "libucrtd.lib",
    ] {
        println!("cargo:rustc-link-arg-tests=/NODEFAULTLIB:{lib}");
    }
    for lib in ["libcmt.lib", "libvcruntime.lib", "ucrt.lib"] {
        println!("cargo:rustc-link-arg-tests=/DEFAULTLIB:{lib}");
    }
}

/// tauri-build 2.7.1 `override_msvcrt_lib`: a COFF object with one empty `.drectve`
/// section, after its two machine-type bytes.
const STUB_BODY: &[u8] = &[
    1, 0, 94, 3, 96, 98, 60, 0, 0, 0, 1, 0, 0, 0, 0, 0, 132, 1, 46, 100, 114, 101, 99, 116, 118,
    101, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    10, 16, 0, 46, 100, 114, 101, 99, 116, 118, 101, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 4, 0, 0, 0,
];
