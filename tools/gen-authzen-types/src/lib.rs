//! Emits the Rust SDK's AuthZEN wire types from the platform's canonical
//! surface artifact.
//!
//! This crate is a build tool, not part of the published SDK. It lives in the
//! SDK repository rather than in the platform so this repository's CI can
//! regenerate and diff without a private repository in its dependency path.
//! The four sibling SDKs each write their own emitter against the SAME
//! artifact; the platform reduces its JSON Schema once, so no emitter
//! re-implements `$ref` resolution or the required/optional rule.
//!
//! # The generated file is committed
//!
//! A consumer running `cargo add axonflow-sdk-rust` must receive working types
//! without running a generator, so the output is committed. A committed
//! generated file is only worth anything if something proves it is the output
//! of the current input, which `tests/authzen_generated_types_are_current.rs`
//! does: it regenerates in memory and compares bytes, so editing either the
//! artifact or the generated file without the other fails CI.

mod emit;
mod surface;

pub use emit::{emit, output_path, surface_path};
pub use surface::{parse_surface, Surface, SurfaceError};

/// Reads the artifact at `surface_path()` relative to `root` and renders the
/// file that belongs at `output_path()`.
///
/// One function so the binary and the SDK's own regeneration test cannot drift
/// into two slightly different pipelines - which is the failure that would let
/// `--check` pass while the committed file was stale.
pub fn render(root: &std::path::Path) -> Result<String, SurfaceError> {
    let path = root.join(surface_path());
    let raw = std::fs::read(&path)
        .map_err(|e| SurfaceError(format!("reading {}: {e}", path.display())))?;
    let surface = parse_surface(&raw)?;
    emit(&surface)
}
