//! The emitter's own tests.
//!
//! These are about the EMITTER, not about the AuthZEN surface: they feed it
//! artifacts the real one is not, and assert it refuses rather than generating
//! something plausible. The check that the committed output matches the real
//! artifact lives in the SDK crate, in
//! `tests/authzen_generated_types_are_current.rs`.

use axonflow_authzen_codegen::{emit, parse_surface};

/// A minimal artifact that parses and emits, as the base for mutations.
fn base_artifact() -> serde_json::Value {
    serde_json::json!({
        "artifact": "axonflow-authzen-surface",
        "artifact_version": 1,
        "profile": "axonflow-authzen-profile-2026-08-29",
        "contract_schema_version": "2026-08-29",
        "source_schema_id": "https://example.invalid/schema.json",
        "source_schema_sha256": "sha256:00",
        "profile_header": "X-Example-Profile",
        "route": {"method": "POST", "path": "/api/v1/example/evaluation"},
        "enums": [{"name": "state", "values": ["ALLOW", "DENY"]}],
        "types": [
            {
                "name": "authzen_leaf",
                "fields": [{"name": "id", "required": true, "type": {"kind": "string"}}]
            },
            {
                "name": "authzen_holder",
                "fields": [
                    {
                        "name": "leaf",
                        "required": false,
                        "type": {"kind": "ref", "ref": "authzen_leaf"},
                        "requires_members": ["id"]
                    },
                    {"name": "props", "required": false, "type": {"kind": "object"}}
                ]
            }
        ]
    })
}

fn render(artifact: &serde_json::Value) -> Result<String, String> {
    let raw = serde_json::to_vec(artifact).expect("the fixture is JSON");
    let surface = parse_surface(&raw).map_err(|e| e.to_string())?;
    emit(&surface).map_err(|e| e.to_string())
}

fn rejection(artifact: serde_json::Value) -> String {
    match render(&artifact) {
        Ok(_) => panic!("the emitter accepted an artifact it should have refused"),
        Err(e) => e,
    }
}

#[test]
fn base_artifact_emits_so_every_rejection_below_is_about_its_own_mutation() {
    // Without this, a mutation test could pass because the BASE was already
    // broken - the rejection would be real and the mutation irrelevant.
    let out = render(&base_artifact()).expect("the base artifact emits");
    assert!(out.contains("pub struct AuthZenLeaf"));
    assert!(out.contains("pub struct AuthZenHolder"));
}

#[test]
fn an_artifact_member_the_emitter_does_not_understand_is_refused() {
    let mut a = base_artifact();
    a["types"][0]["fields"][0]["max_length"] = serde_json::json!(64);
    let e = rejection(a);
    assert!(e.contains("max_length"), "{e}");
}

#[test]
fn a_reference_to_an_undeclared_type_is_refused() {
    let mut a = base_artifact();
    a["types"][1]["fields"][0]["type"]["ref"] = serde_json::json!("authzen_missing");
    let e = rejection(a);
    assert!(e.contains("authzen_missing"), "{e}");
}

#[test]
fn a_reference_to_an_undeclared_enum_is_refused() {
    let mut a = base_artifact();
    a["types"][0]["fields"][0]["type"] = serde_json::json!({"kind": "enum", "enum": "nope"});
    let e = rejection(a);
    assert!(e.contains("nope"), "{e}");
}

#[test]
fn an_unsupported_type_kind_is_refused_rather_than_rendered_as_anything() {
    let mut a = base_artifact();
    a["types"][0]["fields"][0]["type"] = serde_json::json!({"kind": "decimal"});
    let e = rejection(a);
    assert!(e.contains("decimal"), "{e}");
}

#[test]
fn a_duplicate_type_name_is_refused() {
    let mut a = base_artifact();
    let dup = a["types"][0].clone();
    a["types"].as_array_mut().unwrap().push(dup);
    let e = rejection(a);
    assert!(e.contains("twice"), "{e}");
}

#[test]
fn a_duplicate_enum_value_is_refused() {
    let mut a = base_artifact();
    a["enums"][0]["values"] = serde_json::json!(["ALLOW", "ALLOW"]);
    let e = rejection(a);
    assert!(e.contains("twice"), "{e}");
}

#[test]
fn an_exactly_one_of_group_naming_a_field_that_does_not_exist_is_refused() {
    let mut a = base_artifact();
    a["types"][1]["exactly_one_of"] = serde_json::json!([["leaf", "ghost"]]);
    let e = rejection(a);
    assert!(e.contains("ghost"), "{e}");
}

#[test]
fn an_exactly_one_of_group_with_a_single_member_is_refused() {
    // A one-member group is not a constraint, it is a required field written in
    // a way that reads as a choice.
    let mut a = base_artifact();
    a["types"][1]["exactly_one_of"] = serde_json::json!([["leaf"]]);
    let e = rejection(a);
    assert!(e.contains("exactly-one-of group"), "{e}");
}

#[test]
fn requires_members_is_checked_against_the_referenced_type_not_the_declaring_one() {
    // `requires_members` names a member of the type the field POINTS AT. A typo
    // there emits a validator reading a field that does not exist, which fails
    // as a compile error in generated code rather than as a statement about the
    // artifact.
    let mut a = base_artifact();
    a["types"][1]["fields"][0]["requires_members"] = serde_json::json!(["identifier"]);
    let e = rejection(a);
    assert!(e.contains("identifier"), "{e}");
    assert!(e.contains("authzen_leaf"), "{e}");
}

#[test]
fn the_route_and_header_reach_the_output_as_the_constants_the_client_calls() {
    // The generated client sends to AUTHZEN_PATH with AUTHZEN_PROFILE_HEADER;
    // both must come from the artifact, never from a literal in the emitter.
    let out = render(&base_artifact()).expect("the base artifact emits");
    assert!(
        out.contains("pub const AUTHZEN_PATH: &str = \"/api/v1/example/evaluation\";"),
        "{out}"
    );
    assert!(
        out.contains("pub const AUTHZEN_PROFILE_HEADER: &str = \"X-Example-Profile\";"),
        "{out}"
    );
}

#[test]
fn an_artifact_without_a_route_is_refused_rather_than_defaulted() {
    // A client with nowhere to send a request is not a client; the member is
    // required, and its absence is the currency gate refusing an old artifact.
    let mut a = base_artifact();
    a.as_object_mut().unwrap().remove("route");
    let e = rejection(a);
    assert!(e.contains("route"), "{e}");
}

#[test]
fn an_artifact_without_a_profile_header_is_refused_rather_than_defaulted() {
    let mut a = base_artifact();
    a.as_object_mut().unwrap().remove("profile_header");
    let e = rejection(a);
    assert!(e.contains("profile_header"), "{e}");
}

#[test]
fn a_route_that_is_not_post_or_not_absolute_is_refused() {
    for (method, path) in [
        ("GET", "/api/v1/example/evaluation"),
        ("POST", "api/v1/example/evaluation"),
        ("POST", "/api/v1/example/evaluation/"),
    ] {
        let mut a = base_artifact();
        a["route"] = serde_json::json!({"method": method, "path": path});
        let e = rejection(a);
        assert!(
            e.contains("want POST and an absolute path"),
            "{method} {path}: {e}"
        );
    }
}

#[test]
fn a_profile_header_that_is_not_a_header_name_is_refused() {
    for header in ["", "X-Example Profile", "X-Example-Profile:"] {
        let mut a = base_artifact();
        a["profile_header"] = serde_json::json!(header);
        let e = rejection(a);
        assert!(e.contains("is not a header name"), "{header:?}: {e}");
    }
}

#[test]
fn an_artifact_that_is_not_this_surface_is_refused() {
    let mut a = base_artifact();
    a["artifact"] = serde_json::json!("something-else");
    let e = rejection(a);
    assert!(e.contains("something-else"), "{e}");
}

#[test]
fn a_future_artifact_format_version_is_refused_rather_than_generated_through() {
    let mut a = base_artifact();
    a["artifact_version"] = serde_json::json!(2);
    let e = rejection(a);
    assert!(e.contains("deliberate migration"), "{e}");
}

#[test]
fn an_object_member_becomes_the_three_valued_bag_and_never_a_raw_json_value() {
    // The one emission rule with a security argument behind it. A JSON `object`
    // in this artifact is a bag of attributes the CALLER resolved, and
    // `serde_json::Value` has no way to say "the source could not answer" - so
    // rendering it as one would collapse absent and unknown at the type level,
    // before any code had a chance to keep them apart.
    let out = render(&base_artifact()).expect("emits");
    assert!(out.contains("pub props: AttributeMap"), "{out}");
    assert!(
        !out.contains("serde_json::Value"),
        "an object member was rendered as a raw JSON value"
    );
}

#[test]
fn every_declared_type_and_enum_reaches_the_output() {
    // The declared-but-never-emitted class, asserted directly rather than
    // inferred from the file compiling: a type the emitter skipped still
    // compiles, it is just missing.
    let a = base_artifact();
    let out = render(&a).expect("emits");
    for t in a["types"].as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let rust = format!(
            "pub struct AuthZen{}",
            name.trim_start_matches("authzen_")
                .split('_')
                .map(|p| {
                    let mut c = p.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<String>()
        );
        assert!(out.contains(&rust), "{name} did not reach the output");
    }
    assert!(out.contains("pub enum AuthZenState"), "{out}");
}

#[test]
fn every_emitted_enum_is_non_exhaustive() {
    // The byte-comparison gate cannot catch this on its own: deleting the
    // attribute from the emitter and regenerating leaves the committed file
    // exactly what the emitter now produces, so `--check` is green and the
    // public surface has silently become breaking-on-extension. The property
    // is asserted HERE, against the emitter's output, for that reason.
    //
    // Scanned by RENDERING rather than by naming the six enums the current
    // artifact declares: a seventh added tomorrow would be invisible to a list.
    let out = render(&base_artifact()).expect("emits");
    let lines: Vec<&str> = out.lines().collect();
    let mut seen = 0;
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with("pub enum ") {
            continue;
        }
        seen += 1;
        assert!(
            i > 0 && lines[i - 1] == "#[non_exhaustive]",
            "{line} is not preceded by #[non_exhaustive]; \
             a match over its variants plus Unknown(_) is exhaustive today and \
             breaks downstream the moment the artifact gains a value"
        );
    }
    assert!(seen > 0, "no `pub enum` reached the output at all:\n{out}");
}

#[test]
fn exactly_one_emitted_type_is_lenient_about_unknown_members() {
    // The one special case in the emitter, pinned so it cannot silently widen or
    // vanish. Strictness on a DECISION stops a caller acting on a partial
    // reading; strictness on the DIAGNOSTIC costs the caller the code and the
    // pointer, which is all a refusal is. If the artifact ever renames the
    // refusal type, this goes red rather than quietly making everything strict
    // again.
    let out = render(&base_artifact()).expect("emits");
    let lenient: Vec<&str> = out
        .split("#[derive(")
        .skip(1)
        .filter(|chunk| !chunk.contains("#[serde(deny_unknown_fields)]"))
        .filter_map(|chunk| chunk.split("pub struct ").nth(1))
        .filter_map(|rest| rest.split_whitespace().next())
        .collect();
    // The base fixture declares no refusal type, so every struct is strict.
    assert!(
        lenient.is_empty(),
        "these types are lenient and should not be: {lenient:?}"
    );

    // And with a refusal type declared, exactly that one is lenient.
    let mut with_refusal = base_artifact();
    let refusal = serde_json::json!({
        "name": "authzen_error",
        "fields": [{"name": "code", "required": true, "type": {"kind": "string"}}]
    });
    with_refusal["types"].as_array_mut().unwrap().push(refusal);
    let out = render(&with_refusal).expect("emits");
    let lenient: Vec<&str> = out
        .split("#[derive(")
        .skip(1)
        .filter(|chunk| !chunk.contains("#[serde(deny_unknown_fields)]"))
        .filter_map(|chunk| chunk.split("pub struct ").nth(1))
        .filter_map(|rest| rest.split_whitespace().next())
        .collect();
    assert_eq!(lenient, vec!["AuthZenError"]);
}

#[test]
fn generation_is_deterministic_over_repeated_runs() {
    // A leaked map ordering would make the SDK's "is the committed file
    // current?" check fail on unrelated pull requests until somebody deleted it
    // as flaky. Sixteen runs, because one repetition proves nothing about an
    // ordering that happens to be stable within a process.
    let a = base_artifact();
    let first = render(&a).expect("emits");
    for i in 1..16 {
        let again = render(&a).expect("emits");
        assert_eq!(first, again, "emission {i} differed from the first");
    }
}

#[test]
fn a_field_rename_changes_the_output_so_a_drift_cannot_pass_the_byte_comparison() {
    // The survivor case for the regeneration guard: it proves the check CAN go
    // red. A guard that has only ever been observed passing is a guard nobody
    // has established is connected to anything.
    let base = render(&base_artifact()).expect("emits");
    let mut drifted = base_artifact();
    drifted["types"][1]["fields"][1]["name"] = serde_json::json!("attributes");
    let after = render(&drifted).expect("emits");
    assert_ne!(base, after);
    assert!(after.contains("pub attributes: AttributeMap"), "{after}");
    assert!(!after.contains("pub props: AttributeMap"));
}
