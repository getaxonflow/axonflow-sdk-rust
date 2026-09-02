//! Build script: capture the compiling toolchain's version so the SDK
//! heartbeat can report a REAL `runtime_version`.
//!
//! Rust has no runtime equivalent of Go's `runtime.Version()` or Python's
//! `platform.python_version()` — the compiler is not present at run time. The
//! only honest way to report the toolchain is to record it while the compiler
//! IS present, which is here.
//!
//! Before this script the crate sent the literal `"rustc-stable"`, which made
//! the `runtime_version` dimension useless for every Rust row in the telemetry
//! warehouse (see axonflow-sdk-rust#88 item 5).
//!
//! Contract with `heartbeat::runtime_version_str`:
//!
//! * On success this sets `AXONFLOW_RUSTC_VERSION` to the VERBATIM first line
//!   of `rustc --version`, e.g. `rustc 1.95.0 (59807616e 2026-04-14)`. No
//!   parsing happens here — `build.rs` cannot be unit-tested, so normalisation
//!   lives in the crate where `normalize_rustc_version` has a test matrix.
//! * On any failure the variable is NOT set, `option_env!` yields `None`, and
//!   the heartbeat reports `unknown`. It never falls back to a fabricated
//!   literal: a wrong-but-plausible value is worse than an honest absence.
//!
//! `RUSTC` is honoured because cargo sets it to the actual compiler in use
//! (rustup shims, `cross`, sccache wrappers, distro toolchains), which may not
//! be whatever `rustc` resolves to on `PATH`.

use std::process::Command;

fn main() {
    // Re-run when the script itself changes, and when cargo points us at a
    // different compiler. Neither covers "same RUSTC path, upgraded in place",
    // so a stale value is possible after an in-place toolchain upgrade without
    // a clean build. That is a staleness bound on an analytics dimension, not
    // a correctness bug, and the alternative (rerun-if-changed on the rustc
    // binary) breaks reproducible builds in sandboxed environments.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=RUSTC");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());

    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(version) = version {
        // A newline in the value would terminate the cargo directive and let
        // the rest of the string be interpreted as a further instruction. The
        // `.trim()` above removes the trailing newline; this guards the
        // pathological case of a wrapper that prints several lines.
        let first_line = version.lines().next().unwrap_or_default();
        if !first_line.is_empty() {
            println!("cargo:rustc-env=AXONFLOW_RUSTC_VERSION={first_line}");
        }
    }
}
