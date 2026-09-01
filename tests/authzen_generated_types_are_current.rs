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

/// The digest of the vendored artifact, as copied from
/// `enterprise main:platform/decision/surface/authzen-surface.json`.
///
/// WHAT THIS PINS, AND WHAT IT DOES NOT. It makes an edit to the vendored
/// artifact a deliberate TWO-file change with a visible digest in the diff.
/// Without it, "edit the artifact and regenerate" leaves every check green while
/// this SDK has silently forked from the platform - the currency test only
/// proves the types match whatever the artifact currently says.
///
/// It does NOT prove the artifact still matches the platform's: nothing in
/// `cargo test` can reach a private repository. Re-vendoring is a wholesale copy
/// plus this constant, and the diff is where a reviewer sees it. Verify it by
/// hand with `shasum -a 256 testdata/authzen-surface.json`.
const VENDORED_ARTIFACT_SHA256: &str =
    "7f768b8ad0d6278d3531e1410decad172459808ebda627da44dca5bb4c9f36f8";

#[test]
fn the_vendored_artifact_is_the_bytes_this_sdk_was_generated_against() {
    let raw = std::fs::read(sdk_root().join(axonflow_authzen_codegen::surface_path()))
        .expect("the vendored artifact is readable");
    assert_eq!(
        sha256_hex(&raw),
        VENDORED_ARTIFACT_SHA256,
        "the vendored contract artifact changed. It is a wholesale copy of the platform's \
         canonical file; if you re-vendored it deliberately, update VENDORED_ARTIFACT_SHA256 in \
         the same change so the digest is visible in the diff."
    );
}

/// SHA-256, written out rather than pulled in.
///
/// Adding a dependency to a published SDK's graph - carried by every consumer
/// who audits it - to hash one 24 KB file in one test is the wrong trade. The
/// digest is the standard one on purpose: a reviewer checks the constant above
/// with `shasum -a 256`, which no cheaper fingerprint would let them do.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for (i, word) in w.iter().enumerate() {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(*word);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (slot, value) in h.iter_mut().zip(v.iter()) {
            *slot = slot.wrapping_add(*value);
        }
    }
    h.iter().map(|w| format!("{w:08x}")).collect()
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
