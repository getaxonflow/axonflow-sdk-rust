//! The emitter: artifact in, `src/authzen/types_gen.rs` out.
//!
//! # Why the types are generated rather than written
//!
//! The AuthZEN surface ships in five SDKs. Hand-transcribing the same twenty
//! shapes five times produces five slightly different opinions about which
//! fields are optional, and the resulting drift does not look like a bug: it
//! looks like one SDK marking a field required that the others mark optional,
//! discovered by a customer whose request one SDK sends happily and another
//! rejects. The platform reduces its canonical JSON Schema to
//! `testdata/authzen-surface.json`, every SDK generates from that one file, and
//! each repository's CI regenerates and diffs.
//!
//! # Two rules this emitter carries that a `#[derive]` cannot
//!
//! 1. The envelope's exactly-one-of, and the singular member's own required
//!    set. Both are properties of a POSITION rather than of a type, so no field
//!    attribute can express them.
//! 2. `kind: "object"` is not `serde_json::Value`. In this artifact every
//!    `object` member is a bag of attributes the CALLER resolved from somewhere
//!    else - `subject.properties`, `context` - and the type for a resolved
//!    attribute has three states, not two. See [`crate`] docs and
//!    `src/authzen/attribute.rs`.

use crate::surface::{Field, Surface, SurfaceError, Type, TypeRef};
use std::fmt::Write as _;

const GENERATOR: &str = "tools/gen-authzen-types";

/// The one type that must NOT reject an unknown member.
///
/// Every other generated type carries `deny_unknown_fields`, because an unknown
/// member in a DECISION is a server speaking a profile this build does not
/// understand, and reading the rest is acting on a partial interpretation of an
/// authorization decision.
///
/// The refusal document is the opposite case, and getting it wrong costs the
/// caller the whole diagnostic. It is not a decision - it is the server telling
/// you which member to fix - and refusing to decode it because the server added
/// a `retry_after` collapses a typed refusal carrying a code and a JSON Pointer
/// into an opaque transport error with neither. The Go reference gets this
/// right by decoding refusals leniently and reserving strictness for the
/// success path; an earlier version of this emitter did not, and a refusal with
/// one extra member arrived as `Transport` instead of `Refused`.
///
/// Named here rather than inferred, so a reader looking for "why is this one
/// different" finds the argument next to the exception.
const LENIENT_REFUSAL_TYPE: &str = "authzen_error";

/// rustfmt's `max_width`.
///
/// The emitter matches rustfmt rather than deferring to it. Shelling out to
/// `rustfmt` would make the committed file a function of whichever toolchain
/// last ran, so a rustfmt release would turn the "is the committed file
/// current?" check red on unrelated pull requests - and rustfmt's own
/// exclusion mechanisms (`rustfmt.toml`'s `ignore`, `#![rustfmt::skip]`) are
/// both nightly-only, measured on stable 1.95. So the emitter emits what
/// rustfmt would emit, and `cargo fmt --all --check` in CI is what keeps it
/// honest: if a future rustfmt disagrees, that job says so in a diff rather
/// than the byte comparison saying so in a puzzle.
const MAX_WIDTH: usize = 100;

/// rustfmt's `struct_lit_width`: a struct literal whose members fit in this
/// many columns is written on one line.
const STRUCT_LIT_WIDTH: usize = 18;

/// Whether a rendered line is short enough that rustfmt would leave it whole.
fn fits(line: &str) -> bool {
    line.chars().count() <= MAX_WIDTH
}

/// Renders one call argument the way rustfmt would: on its own line when it
/// fits at `indent`, wrapped inside the call's parentheses when it does not.
///
/// The returned string always ends in a newline, so a caller splices it into a
/// multi-line call without deciding anything about spacing.
fn render_arg(indent: usize, arg: &str) -> String {
    render_arg_at(indent, indent, arg)
}

/// [`render_arg`], for a fragment that will be re-indented before it lands.
///
/// `final_indent` is the column the line will actually sit at, and decides
/// whether it fits; `local_indent` is the padding written into the fragment
/// now. Deciding from the local indent instead would let a line that is
/// comfortably short in the fragment overflow once the guard is wrapped around
/// it, and the emitter would silently stop being a rustfmt fixpoint.
fn render_arg_at(final_indent: usize, local_indent: usize, arg: &str) -> String {
    let pad = " ".repeat(local_indent);
    let one_line = format!("{pad}{arg},");
    let final_line = format!("{}{arg},", " ".repeat(final_indent));
    if fits(&final_line) {
        return format!("{one_line}\n");
    }
    // rustfmt breaks a `format!(...)` by moving its own argument in one level.
    let inner_pad = " ".repeat(local_indent + 4);
    let stripped = arg
        .strip_prefix("format!(")
        .and_then(|rest| rest.strip_suffix(')'));
    match stripped {
        Some(inner) => format!("{pad}format!(\n{inner_pad}{inner}\n{pad}),\n"),
        None => format!("{one_line}\n"),
    }
}

/// How far a member's check is indented: a required member is read directly, an
/// optional one sits inside an `if let Some(v)`.
fn guard_indent(f: &Field) -> usize {
    if f.required {
        8
    } else {
        12
    }
}

/// Emits `body` inside a `validate` fn, with `v` bound to the member's value.
///
/// A required member is read directly; an optional one is guarded by
/// `if let Some(v)`. Wrapping a required member in `Some(&self.x)` so both
/// paths could share one shape reads as a value that might be missing when it
/// cannot be - and clippy says so out loud.
fn write_member_guard(b: &mut String, f: &Field, fname: &str, body: &str) {
    let pad = " ".repeat(guard_indent(f));
    if f.required {
        let _ = writeln!(b, "        let v = &self.{fname};");
        for line in body.lines() {
            let _ = writeln!(b, "{pad}{line}");
        }
    } else {
        let _ = writeln!(b, "        if let Some(v) = self.{fname}.as_ref() {{");
        for line in body.lines() {
            let _ = writeln!(b, "{pad}{line}");
        }
        let _ = writeln!(b, "        }}");
    }
}
const SURFACE_PATH: &str = "testdata/authzen-surface.json";
const OUTPUT_PATH: &str = "src/authzen/types_gen.rs";

/// The path, relative to the SDK root, the emitter writes.
pub fn output_path() -> &'static str {
    OUTPUT_PATH
}

/// The path, relative to the SDK root, the emitter reads.
pub fn surface_path() -> &'static str {
    SURFACE_PATH
}

fn err<T>(msg: impl Into<String>) -> Result<T, SurfaceError> {
    Err(SurfaceError(msg.into()))
}

/// Renders the whole file.
pub fn emit(s: &Surface) -> Result<String, SurfaceError> {
    if s.types.is_empty() || s.enums.is_empty() {
        return err(format!(
            "the artifact describes {} type(s) and {} enum(s); generating from an empty surface \
             would silently produce an empty SDK",
            s.types.len(),
            s.enums.len()
        ));
    }
    if s.artifact != "axonflow-authzen-surface" {
        return err(format!(
            "{SURFACE_PATH} is not an AuthZEN surface artifact (artifact={:?})",
            s.artifact
        ));
    }
    if s.artifact_version != 1 {
        return err(format!(
            "artifact format version {} is not supported by this emitter; a format change is a \
             deliberate migration, not something to generate through",
            s.artifact_version
        ));
    }

    let mut b = String::new();
    write_header(&mut b, s);
    for e in &s.enums {
        emit_enum(&mut b, e);
    }
    for t in &s.types {
        emit_type(&mut b, t)?;
    }
    // Exactly one trailing newline. Every item is emitted with a blank line
    // after it, which leaves two at the end of the file - and rustfmt strips
    // the second, which would make the emitter permanently one byte away from
    // being its own fixpoint.
    Ok(format!("{}\n", b.trim_end()))
}

fn write_header(b: &mut String, s: &Surface) {
    let _ = write!(
        b,
        "// Code generated by {GENERATOR}. DO NOT EDIT.\n\
         //\n\
         // Source: {SURFACE_PATH}\n\
         //   artifact:        {} v{}\n\
         //   profile:         {}\n\
         //   contract schema: {}\n\
         //   schema digest:   {}\n\
         //\n\
         // Regenerate with:\n\
         //\n\
         //     cargo run -p axonflow-authzen-codegen\n\
         //\n\
         // Editing this file by hand is pointless: tests/authzen_generated_types_are_current.rs\n\
         // regenerates it in memory and compares bytes, so a hand edit fails CI on the next run.\n\
         \n\
         use super::attribute::AttributeMap;\n\
         use serde::{{Deserialize, Serialize}};\n\
         use std::collections::BTreeMap;\n\
         \n",
        s.artifact,
        s.artifact_version,
        s.profile,
        s.contract_schema_version,
        s.source_schema_sha256
    );

    let _ = write!(
        b,
        "/// The profile a Policy Enforcement Point negotiates to receive anything\n\
         /// beyond the boolean decision.\n\
         ///\n\
         /// AuthZEN 1.0's response is a bare boolean. The four-valued state, the\n\
         /// obligations, the approval challenge and the safe reason code all ride in\n\
         /// the response context and are returned ONLY to a caller that asked for them\n\
         /// by version, because handing a partial interpretation to a caller that\n\
         /// cannot act on it is worse than handing it the boolean it understands.\n\
         pub const AUTHZEN_PROFILE_V1: &str = {:?};\n\
         \n\
         /// The contract version these types were generated from. It is the value the\n\
         /// server echoes in [`AuthZenResponseContext::schema_version`].\n\
         pub const AUTHZEN_CONTRACT_SCHEMA_VERSION: &str = {:?};\n\
         \n",
        s.profile, s.contract_schema_version
    );
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

fn emit_enum(b: &mut String, e: &crate::surface::Enum) {
    let name = type_name(&e.name);
    if !e.doc.is_empty() {
        write_doc(b, "", &e.doc);
        let _ = writeln!(b, "///");
    }
    let _ = write!(
        b,
        "/// A closed set of values the server may send.\n\
         ///\n\
         /// The `Unknown` variant is not a failure mode, it is a ROUND TRIP: a value a\n\
         /// newer server added after this build survives decode, re-encode and logging\n\
         /// intact instead of collapsing onto a neighbouring constant. It is, however,\n\
         /// a reason not to branch on the value as though it were one of the known\n\
         /// ones - use [`{name}::is_known`].\n"
    );
    let _ = write!(
        b,
        "#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]\n\
         #[serde(from = \"String\", into = \"String\")]\n\
         pub enum {name} {{\n"
    );
    for v in &e.values {
        let _ = writeln!(b, "    /// `{v}`");
        let _ = writeln!(b, "    {},", variant_name(v));
    }
    let _ = write!(
        b,
        "    /// A value this build does not know, carried verbatim.\n\
         \x20   Unknown(String),\n\
         }}\n\
         \n"
    );

    // KNOWN_WIRE_VALUES rather than an `all()` returning the enum: the enum
    // carries a String variant, so a `&'static [Self]` cannot be promoted, and
    // a Vec-returning accessor would allocate on every call for a list that
    // never changes. The wire strings are what a diagnostic wants anyway.
    let _ = write!(
        b,
        "impl {name} {{\n\
         \x20   /// Every wire value this build knows, in artifact order.\n\
         \x20   ///\n\
         \x20   /// This is the set a refusal names in its `supported` list, so it is the\n\
         \x20   /// list a diagnostic should print rather than one written beside it.\n"
    );
    let head = "    pub const KNOWN_WIRE_VALUES: &'static [&'static str] = &[";
    let joined: Vec<String> = e.values.iter().map(|v| format!("{v:?}")).collect();
    let one_line = format!("{head}{}];", joined.join(", "));
    if fits(&one_line) {
        let _ = writeln!(b, "{one_line}");
    } else {
        let _ = writeln!(b, "{head}");
        for v in &joined {
            let _ = writeln!(b, "        {v},");
        }
        let _ = writeln!(b, "    ];");
    }
    let _ = write!(
        b,
        "\n\
         \x20   /// The wire value.\n\
         \x20   pub fn as_str(&self) -> &str {{\n\
         \x20       match self {{\n"
    );
    for v in &e.values {
        let _ = writeln!(b, "            Self::{} => {v:?},", variant_name(v));
    }
    let _ = write!(
        b,
        "            Self::Unknown(v) => v.as_str(),\n\
         \x20       }}\n\
         \x20   }}\n\
         \n\
         \x20   /// Whether this is a value this build knows.\n\
         \x20   ///\n\
         \x20   /// A false result is not necessarily an error - a newer server may send a\n\
         \x20   /// value added after this SDK was built. It IS a reason not to treat it as\n\
         \x20   /// equivalent to any known value.\n\
         \x20   pub fn is_known(&self) -> bool {{\n\
         \x20       !matches!(self, Self::Unknown(_))\n\
         \x20   }}\n\
         }}\n\
         \n"
    );

    let _ = write!(
        b,
        "impl From<String> for {name} {{\n\
         \x20   fn from(v: String) -> Self {{\n\
         \x20       match v.as_str() {{\n"
    );
    for v in &e.values {
        let _ = writeln!(b, "            {v:?} => Self::{},", variant_name(v));
    }
    let _ = write!(
        b,
        "            _ => Self::Unknown(v),\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n\
         \n\
         impl From<{name}> for String {{\n\
         \x20   fn from(v: {name}) -> Self {{\n\
         \x20       v.as_str().to_string()\n\
         \x20   }}\n\
         }}\n\
         \n\
         impl std::fmt::Display for {name} {{\n\
         \x20   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n\
         \x20       f.write_str(self.as_str())\n\
         \x20   }}\n\
         }}\n\
         \n"
    );
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

fn emit_type(b: &mut String, t: &Type) -> Result<(), SurfaceError> {
    let name = type_name(&t.name);
    if t.doc.is_empty() {
        let _ = writeln!(b, "/// Part of the AuthZEN wire surface.");
    } else {
        write_doc(b, "", &t.doc);
    }

    let derives = if t.fields.iter().any(|f| f.required) {
        // A type with a required member has no meaningful default: `Default`
        // would hand back a value the server refuses, and `..Default::default()`
        // in a struct literal would silently leave it there.
        "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]"
    } else {
        "#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]"
    };
    let _ = writeln!(b, "{derives}");
    if t.name != LENIENT_REFUSAL_TYPE {
        let _ = writeln!(b, "#[serde(deny_unknown_fields)]");
    }
    let _ = writeln!(b, "pub struct {name} {{");
    for (i, f) in t.fields.iter().enumerate() {
        if i > 0 {
            let _ = writeln!(b);
        }
        if !f.doc.is_empty() {
            write_doc(b, "    ", &f.doc);
        }
        if !f.r#const.is_empty() {
            let _ = writeln!(
                b,
                "    /// The only value the server sends is `{}`.",
                f.r#const
            );
        }
        for attr in serde_attrs(f) {
            let _ = writeln!(b, "    {attr}");
        }
        let _ = writeln!(b, "    pub {}: {},", field_name(&f.name), rust_type(f)?);
    }
    let _ = writeln!(b, "}}\n");

    emit_new(b, &name, t)?;
    emit_validate(b, &name, t);
    Ok(())
}

fn serde_attrs(f: &Field) -> Vec<String> {
    // A required member is never skipped when empty. An empty required array is
    // a violation the validator reports by name; omitting it on the wire would
    // turn that violation into a DIFFERENT one (absent) at the server, and the
    // caller would be pointed at the wrong problem.
    if f.required {
        return vec![];
    }
    let skip = match f.type_ref.kind.as_str() {
        "object" => "AttributeMap::is_empty",
        "array" => "Vec::is_empty",
        "map" => "BTreeMap::is_empty",
        _ => "Option::is_none",
    };
    vec![format!(
        "#[serde(default, skip_serializing_if = \"{skip}\")]"
    )]
}

fn emit_new(b: &mut String, name: &str, t: &Type) -> Result<(), SurfaceError> {
    let required: Vec<&Field> = t.fields.iter().filter(|f| f.required).collect();
    if required.is_empty() {
        // Every member optional: `Default` is derived and is the constructor.
        return Ok(());
    }
    let mut args = Vec::new();
    for f in &required {
        let arg = match f.type_ref.kind.as_str() {
            "string" => "impl Into<String>".to_string(),
            _ => rust_type(f)?,
        };
        args.push(format!("{}: {arg}", field_name(&f.name)));
    }
    let _ = write!(
        b,
        "impl {name} {{\n\
         \x20   /// Builds a value carrying every member the contract requires.\n\
         \x20   ///\n\
         \x20   /// Optional members start empty and are set by assignment, so a member\n\
         \x20   /// added to the contract later becomes a compile error at NO call site\n\
         \x20   /// that did not want it - which is the point of not deriving `Default`\n\
         \x20   /// here.\n"
    );
    let one_line = format!("    pub fn new({}) -> Self {{", args.join(", "));
    if fits(&one_line) {
        let _ = writeln!(b, "{one_line}");
    } else {
        let _ = writeln!(b, "    pub fn new(");
        for a in &args {
            let _ = writeln!(b, "        {a},");
        }
        let _ = writeln!(b, "    ) -> Self {{");
    }
    let mut inits = Vec::new();
    for f in &t.fields {
        let fname = field_name(&f.name);
        if f.required {
            if f.type_ref.kind == "string" {
                inits.push(format!("{fname}: {fname}.into()"));
            } else {
                // Field-name shorthand, because `quorum: quorum` is a clippy
                // finding and generated code has to pass the same lint gate as
                // everything else - a generator that emits warnings teaches the
                // repository to tolerate them.
                inits.push(fname);
            }
        } else {
            inits.push(format!("{fname}: {}", empty_value(&f.type_ref)));
        }
    }
    let joined = inits.join(", ");
    if joined.chars().count() <= STRUCT_LIT_WIDTH {
        let _ = writeln!(b, "        Self {{ {joined} }}");
    } else {
        let _ = writeln!(b, "        Self {{");
        for init in &inits {
            let _ = writeln!(b, "            {init},");
        }
        let _ = writeln!(b, "        }}");
    }
    let _ = write!(b, "    }}\n}}\n\n");
    Ok(())
}

fn empty_value(tr: &TypeRef) -> &'static str {
    match tr.kind.as_str() {
        "object" => "AttributeMap::new()",
        "array" => "Vec::new()",
        "map" => "BTreeMap::new()",
        _ => "None",
    }
}

/// Renders the checks the type system cannot carry.
///
/// EVERY type gets one, including types with no constraint of their own. A type
/// whose members are all optional still OWNS the validation of those members:
/// skipping it because "there is nothing to check here" is how a parent reports
/// OK while a child carries a violation the server refuses. Go's emitter made
/// exactly that judgement and `AuthZENRequest` - the singular envelope member -
/// ended up with no validator at all, so a subject with no `type` passed local
/// validation and was refused on the wire.
fn emit_validate(b: &mut String, name: &str, t: &Type) {
    let _ = write!(
        b,
        "impl {name} {{\n\
         \x20   /// Reports whether this value carries what the server requires.\n\
         \x20   ///\n\
         \x20   /// `at` is the JSON Pointer this value sits at in the envelope, so a\n\
         \x20   /// refusal names the same member the server would name. Pass `\"\"` at the\n\
         \x20   /// root.\n\
         \x20   pub fn validate(&self, at: &str) -> Result<(), AuthZenError> {{\n"
    );
    let mut wrote = false;

    for group in &t.exactly_one_of {
        wrote = true;
        // One `if` per member rather than a summing expression: the summing
        // form's width grows with the group, so a third member would push the
        // line past rustfmt's column limit and the emitter would stop being a
        // fixpoint for a reason nothing about the group made obvious.
        let _ = writeln!(b, "        let mut present = 0;");
        for m in group {
            let _ = write!(
                b,
                "        if self.{}.is_some() {{\n\
                 \x20           present += 1;\n\
                 \x20       }}\n",
                field_name(m)
            );
        }
        let message = format!(
            "format!(\"exactly one of {} must be present, {{present}} are\")",
            group.join(" or ")
        );
        let _ = write!(
            b,
            "        if present != 1 {{\n\
             \x20           return Err(AuthZenError::new(\n\
             \x20               AuthZenErrorCode::MalformedEnvelope,\n\
             {}\
             \x20           )\n\
             \x20           .at(at));\n\
             \x20       }}\n",
            render_arg(16, &message)
        );
    }

    for f in &t.fields {
        let fname = field_name(&f.name);
        let wire = &f.name;
        let pointer = format!("&format!(\"{{at}}/{wire}\")");

        if f.required {
            match f.type_ref.kind.as_str() {
                "string" => {
                    wrote = true;
                    let _ = write!(
                        b,
                        "        if self.{fname}.is_empty() {{\n\
                         \x20           return Err(AuthZenError::new(\n\
                         \x20               AuthZenErrorCode::IncompleteEvaluation,\n\
                         \x20               \"{wire} is required\",\n\
                         \x20           )\n\
                         \x20           .at({pointer}));\n\
                         \x20       }}\n"
                    );
                }
                "enum" => {
                    wrote = true;
                    let _ = write!(
                        b,
                        "        if self.{fname}.as_str().is_empty() {{\n\
                         \x20           return Err(AuthZenError::new(\n\
                         \x20               AuthZenErrorCode::IncompleteEvaluation,\n\
                         \x20               \"{wire} is required\",\n\
                         \x20           )\n\
                         \x20           .at({pointer}));\n\
                         \x20       }}\n"
                    );
                }
                _ => {}
            }
        }

        // A required string is already refused when empty, so `min_length: 1`
        // on one would emit a branch that can never be taken. Emitting it
        // anyway is not merely noise: an unreachable branch in generated code
        // is a branch no test can cover, and it teaches a reader that the two
        // checks are independent when they are not.
        if f.min_length > 0 && (!f.required || f.min_length > 1) {
            wrote = true;
            let n = f.min_length;
            let body = format!(
                "if v.chars().count() < {n} {{\n\
                 \x20   return Err(AuthZenError::new(\n\
                 \x20       AuthZenErrorCode::IncompleteEvaluation,\n\
                 \x20       \"{wire} needs at least {n} character{}\",\n\
                 \x20   )\n\
                 \x20   .at({pointer}));\n\
                 }}\n",
                if n == 1 { "" } else { "s" }
            );
            write_member_guard(b, f, &fname, &body);
        }

        if !f.r#const.is_empty() {
            wrote = true;
            let c = &f.r#const;
            // The literal is interpolated into a Rust string literal below, so
            // a quote or a backslash in it would close that literal and emit
            // source that does not parse. Refusing is the only safe answer: an
            // escaping routine here would be a second, untested encoder.
            if c.contains('"') || c.contains('\\') {
                let _ = writeln!(
                    b,
                    "        compile_error!(\"const values carrying a quote or backslash are not supported\");"
                );
                continue;
            }
            // The message's own indentation depends on whether the member is
            // guarded, so it is computed from the same predicate rather than
            // written twice.
            let msg_indent = guard_indent(f) + 8;
            let body = format!(
                "if v.as_str() != {c:?} {{\n\
                 \x20   return Err(AuthZenError::new(\n\
                 \x20       AuthZenErrorCode::MalformedEnvelope,\n\
                 {}\
                 \x20   )\n\
                 \x20   .at({pointer})\n\
                 \x20   .supporting([{c:?}]));\n\
                 }}\n",
                render_arg_at(
                    msg_indent,
                    8,
                    &format!("format!(\"{wire} must be {c}, got {{v:?}}\")")
                )
            );
            write_member_guard(b, f, &fname, &body);
        }

        if f.min_items > 0 {
            wrote = true;
            let n = f.min_items;
            // `len() < 1` is a clippy finding; `is_empty()` asks the same
            // question the way the lint wants it asked.
            let empty_check = if n == 1 {
                format!("self.{fname}.is_empty()")
            } else {
                format!("self.{fname}.len() < {n}")
            };
            let _ = write!(
                b,
                "        if {empty_check} {{\n\
                 \x20           return Err(AuthZenError::new(\n\
                 \x20               AuthZenErrorCode::MalformedEnvelope,\n\
                 \x20               \"{wire} needs at least {n} entr{}\",\n\
                 \x20           )\n\
                 \x20           .at({pointer}));\n\
                 \x20       }}\n",
                if n == 1 { "y" } else { "ies" }
            );
        }

        for m in &f.requires_members {
            wrote = true;
            let mn = field_name(m);
            let _ = write!(
                b,
                "        if let Some(v) = self.{fname}.as_ref() {{\n\
                 \x20           if v.{mn}.is_none() {{\n\
                 \x20               return Err(AuthZenError::new(\n\
                 \x20                   AuthZenErrorCode::IncompleteEvaluation,\n\
                 \x20                   \"{wire} has no {m}; it has no shared base to inherit one from\",\n\
                 \x20               )\n\
                 \x20               .at(&format!(\"{{at}}/{wire}/{m}\")));\n\
                 \x20           }}\n\
                 \x20       }}\n"
            );
        }

        // Nested validation. Without it a parent reports OK while a child
        // carries a violation the server refuses.
        match f.type_ref.kind.as_str() {
            "ref" => {
                wrote = true;
                if f.required {
                    let _ = writeln!(b, "        self.{fname}.validate({pointer})?;");
                } else {
                    let _ = write!(
                        b,
                        "        if let Some(v) = self.{fname}.as_ref() {{\n\
                         \x20           v.validate({pointer})?;\n\
                         \x20       }}\n"
                    );
                }
            }
            "array" if f.type_ref.items.as_ref().map(|i| i.kind.as_str()) == Some("ref") => {
                wrote = true;
                let _ = write!(
                    b,
                    "        for (i, v) in self.{fname}.iter().enumerate() {{\n\
                         \x20           v.validate(&format!(\"{{at}}/{wire}/{{i}}\"))?;\n\
                         \x20       }}\n"
                );
            }
            "object" => {
                wrote = true;
                let _ = writeln!(b, "        self.{fname}.validate({pointer})?;");
            }
            _ => {}
        }
    }

    if !wrote {
        let _ = writeln!(b, "        let _ = at;");
    }
    let _ = write!(b, "        Ok(())\n    }}\n}}\n\n");
}

// ---------------------------------------------------------------------------
// Names and types
// ---------------------------------------------------------------------------

fn rust_type(f: &Field) -> Result<String, SurfaceError> {
    let inner = base_type(&f.type_ref)?;
    Ok(match f.type_ref.kind.as_str() {
        // A bag of attributes is never optional: the empty bag IS "the caller
        // supplied none", and wrapping it in an Option would add a fourth state
        // to a member whose values already carry three.
        "object" | "array" | "map" => inner,
        _ if f.required => inner,
        _ => format!("Option<{inner}>"),
    })
}

fn base_type(tr: &TypeRef) -> Result<String, SurfaceError> {
    Ok(match tr.kind.as_str() {
        "string" => "String".to_string(),
        "bool" => "bool".to_string(),
        "int" => "i64".to_string(),
        // NOT serde_json::Value. See the module doc: in this artifact an
        // `object` is a bag of caller-resolved attributes, and a resolved
        // attribute has three states.
        "object" => "AttributeMap".to_string(),
        "enum" => type_name(&tr.r#enum),
        "ref" => type_name(&tr.r#ref),
        "array" => match &tr.items {
            Some(items) => format!("Vec<{}>", base_type(items)?),
            None => return err("an array with no item type"),
        },
        "map" => match &tr.value {
            Some(value) => format!("BTreeMap<String, {}>", base_type(value)?),
            None => return err("a map with no value type"),
        },
        // Never a default type. An unrecognised kind silently rendered as
        // `serde_json::Value` would compile, ship, and accept values the server
        // refuses.
        other => return err(format!("unsupported type kind {other:?}")),
    })
}

/// Maps an artifact name onto this SDK's exported name.
///
/// Every generated type carries the `AuthZen` prefix. It is not decoration:
/// this crate already exports `Obligation` (`types::pep`), and a generated type
/// of the same name would collide in the prelude. Prefixing everything rather
/// than only what collides today keeps the rule mechanical - a future collision
/// does not require choosing a new convention under time pressure.
///
/// `AuthZen`, not `AuthZEN`: Rust's API guidelines treat an acronym as one word
/// (`Uuid`, not `UUID`). Go and Java spell it `AuthZEN` because that is their
/// idiom and, in Java's case, because the wire-shape gate matches the OpenAPI
/// schema names literally. The WIRE is identical in all five; only the local
/// spelling of a type name differs, and a type name is not a wire contract.
fn type_name(artifact_name: &str) -> String {
    format!(
        "AuthZen{}",
        pascal(
            artifact_name
                .strip_prefix("authzen_")
                .unwrap_or(artifact_name)
        )
    )
}

/// Maps a wire member name onto a Rust field name, escaping keywords.
///
/// The escape is `r#type`, not a rename to `kind` or `type_`: `kind` is already
/// a DIFFERENT member of `identifier`, and a trailing underscore would need a
/// `#[serde(rename)]` on every such field - one more thing for a future member
/// to be forgotten in.
fn field_name(wire: &str) -> String {
    let snake = wire.to_ascii_lowercase();
    if is_rust_keyword(&snake) {
        format!("r#{snake}")
    } else {
        snake
    }
}

fn is_rust_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
    )
}

/// Maps an enum's wire value onto a variant name.
///
/// Values are either `lower_snake` (`not_permitted`) or upper (`ALLOW`).
/// Lowercasing first sends both through one rule, so no value needs a special
/// case and no future value can find one missing.
fn variant_name(value: &str) -> String {
    pascal(&value.to_ascii_lowercase())
}

fn pascal(s: &str) -> String {
    let mut out = String::new();
    for part in s.split(['_', '.', '-']) {
        if part.is_empty() {
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Renders the artifact's description as a Rust doc comment.
///
/// The artifact carries prose wrapped for JSON, which is not where these end
/// up, so it is reflowed. rustfmt leaves doc-comment TEXT alone (`wrap_comments`
/// is off by default), so this width is the emitter's decision and stays stable
/// across toolchains.
fn write_doc(b: &mut String, indent: &str, doc: &str) {
    for line in wrap(doc, 74 - indent.len()) {
        if line.is_empty() {
            let _ = writeln!(b, "{indent}///");
        } else {
            let _ = writeln!(b, "{indent}/// {line}");
        }
    }
}

fn wrap(s: &str, width: usize) -> Vec<String> {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut cur = words[0].to_string();
    for w in &words[1..] {
        if cur.chars().count() + 1 + w.chars().count() > width {
            out.push(std::mem::take(&mut cur));
            cur = (*w).to_string();
            continue;
        }
        cur.push(' ');
        cur.push_str(w);
    }
    out.push(cur);
    out
}
