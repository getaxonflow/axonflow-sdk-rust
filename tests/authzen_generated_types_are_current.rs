//! The committed `src/authzen/types_gen.rs` is what the vendored artifact
//! generates.
//!
//! This is the whole reason the generated file may be committed at all. A
//! committed generated file that nothing checks is a hand-written file with a
//! misleading header: it drifts from its input on the first edit, and the
//! header goes on claiming it did not.
//!
//! It lives in `cargo test` rather than in a workflow step so it cannot be
//! forgotten when a workflow is rewritten, and so a contributor sees it fail on
//! their machine before CI does.

use std::path::{Path, PathBuf};

fn sdk_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the crate root whatever directory the test was
    // started from, so this does not depend on the caller's shell.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn committed(root: &Path) -> String {
    let path = root.join(axonflow_authzen_codegen::output_path());
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading the committed types at {}: {e}", path.display()))
}

#[test]
fn the_committed_types_are_what_the_vendored_artifact_generates() {
    let root = sdk_root();
    let rendered = axonflow_authzen_codegen::render(&root).expect("the vendored artifact emits");
    let have = committed(&root);

    if have != rendered {
        let first_difference = have
            .lines()
            .zip(rendered.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n  committed: {a}\n  generated: {b}", i + 1))
            .unwrap_or_else(|| "the files differ in length only".to_string());
        panic!(
            "{} is not what {} generates.\n\n{first_difference}\n\nRegenerate them in the same \
             change:\n  cargo run -p axonflow-authzen-codegen\n\nIf you edited the generated file \
             by hand: edit the emitter instead. If you edited the artifact: it is vendored from \
             the platform's canonical contract and should be replaced wholesale, not patched.",
            axonflow_authzen_codegen::output_path(),
            axonflow_authzen_codegen::surface_path(),
        );
    }
}

#[test]
fn regenerating_repeatedly_produces_the_same_bytes() {
    // Determinism over the REAL artifact, not just the emitter's own fixture.
    // A leaked map ordering here would make the check above fail on pull
    // requests that touched none of this, and the usual response to a check
    // that fails at random is to delete it.
    let root = sdk_root();
    let first = axonflow_authzen_codegen::render(&root).expect("emits");
    for i in 1..16 {
        let again = axonflow_authzen_codegen::render(&root).expect("emits");
        assert_eq!(first, again, "emission {i} differed from the first");
    }
}

#[test]
fn a_field_shape_drift_in_the_artifact_makes_the_check_fail() {
    // The SURVIVOR case. The test above has only ever been observed passing,
    // and a check that has never been seen to fail is a check nobody has
    // established is connected to its subject. This plants a drift of exactly
    // the shape a contract change would have - one member renamed - and asserts
    // the byte comparison notices.
    //
    // In memory: writing the drift to disk would leave the tree mutated if the
    // test were killed part-way, which is a failure mode this repository has
    // been bitten by before.
    let root = sdk_root();
    let raw = std::fs::read(root.join(axonflow_authzen_codegen::surface_path()))
        .expect("the vendored artifact is readable");
    let mut surface = axonflow_authzen_codegen::parse_surface(&raw).expect("it parses");

    let subject = surface
        .types
        .iter_mut()
        .find(|t| t.name == "authzen_subject")
        .expect("the artifact declares authzen_subject");
    let properties = subject
        .fields
        .iter_mut()
        .find(|f| f.name == "properties")
        .expect("authzen_subject declares properties");
    properties.name = "attributes".to_string();

    let drifted = axonflow_authzen_codegen::emit(&surface).expect("the drifted artifact emits");
    let have = committed(&root);

    assert_ne!(
        have, drifted,
        "a renamed member produced byte-identical output, so the regeneration check cannot see a \
         field-shape drift at all"
    );
    assert!(
        drifted.contains("pub attributes: AttributeMap"),
        "the planted drift did not reach the emitted types, so this test proves nothing"
    );
}

#[test]
fn the_vendored_artifact_is_the_contract_the_types_claim_to_come_from() {
    // The generated header names a profile and a schema digest. Those are the
    // strings a support engineer compares against a server's response, so they
    // have to come from the artifact rather than from a constant somebody
    // updated by hand.
    let root = sdk_root();
    let raw = std::fs::read(root.join(axonflow_authzen_codegen::surface_path())).expect("readable");
    let surface = axonflow_authzen_codegen::parse_surface(&raw).expect("parses");
    let have = committed(&root);

    assert!(have.contains(&surface.profile), "the profile is not named");
    assert!(
        have.contains(&surface.source_schema_sha256),
        "the source schema digest is not named"
    );
    assert_eq!(
        surface.profile,
        axonflow_sdk_rust::AUTHZEN_PROFILE_V1,
        "the constant the client negotiates with is not the artifact's profile"
    );
    assert_eq!(
        surface.contract_schema_version,
        axonflow_sdk_rust::AUTHZEN_CONTRACT_SCHEMA_VERSION
    );
}
