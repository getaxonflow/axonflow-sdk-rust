//! The language-neutral AuthZEN surface artifact, as this emitter reads it.
//!
//! These declarations mirror the platform's producer side. They are a SUBSET on
//! purpose: an emitter must fail on an artifact member it does not understand
//! rather than generate around it, which is why [`parse_surface`] rejects
//! unknown members instead of ignoring them. A member the platform added and
//! this SDK silently omitted is the declared-but-never-emitted class arriving
//! through the very generator built to prevent it.

use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt;

/// The whole artifact.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Surface {
    pub artifact: String,
    pub artifact_version: u32,
    pub profile: String,
    pub contract_schema_version: String,
    #[allow(dead_code)]
    pub source_schema_id: String,
    pub source_schema_sha256: String,
    pub enums: Vec<Enum>,
    pub types: Vec<Type>,
}

/// A closed set of string values.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enum {
    pub name: String,
    #[serde(default)]
    pub doc: String,
    pub values: Vec<String>,
}

/// One object shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Type {
    pub name: String,
    #[serde(default)]
    pub doc: String,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub exactly_one_of: Vec<Vec<String>>,
}

/// One member of a type.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(default)]
    pub doc: String,
    pub required: bool,
    #[serde(rename = "type")]
    pub type_ref: TypeRef,
    #[serde(default)]
    pub min_items: usize,
    #[serde(default)]
    pub min_length: usize,
    #[serde(default)]
    pub requires_members: Vec<String>,
    #[serde(default)]
    pub r#const: String,
}

/// A field's type.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeRef {
    pub kind: String,
    #[serde(default)]
    pub r#ref: String,
    #[serde(default)]
    pub r#enum: String,
    #[serde(default)]
    pub items: Option<Box<TypeRef>>,
    #[serde(default)]
    pub value: Option<Box<TypeRef>>,
}

/// Why an artifact could not be read, or does not hang together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceError(pub String);

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SurfaceError {}

fn err<T>(msg: impl Into<String>) -> Result<T, SurfaceError> {
    Err(SurfaceError(msg.into()))
}

/// Decodes the artifact STRICTLY and checks that it hangs together.
///
/// Every reference must resolve inside the document. A dangling ref would
/// otherwise become a Rust type name that does not exist, and the failure would
/// surface as a compile error in generated code rather than as a statement
/// about the artifact.
pub fn parse_surface(raw: &[u8]) -> Result<Surface, SurfaceError> {
    let s: Surface = serde_json::from_slice(raw)
        .map_err(|e| SurfaceError(format!("parsing the surface artifact: {e}")))?;

    let mut types: BTreeSet<&str> = BTreeSet::new();
    for t in &s.types {
        if !types.insert(t.name.as_str()) {
            return err(format!("the artifact declares the type {:?} twice", t.name));
        }
    }
    let mut enums: BTreeSet<&str> = BTreeSet::new();
    for e in &s.enums {
        if !enums.insert(e.name.as_str()) {
            return err(format!("the artifact declares the enum {:?} twice", e.name));
        }
        if e.values.is_empty() {
            return err(format!("enum {:?} has no values", e.name));
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for v in &e.values {
            if !seen.insert(v.as_str()) {
                return err(format!(
                    "enum {:?} declares the value {:?} twice",
                    e.name, v
                ));
            }
        }
    }
    for t in &s.types {
        if t.fields.is_empty() {
            return err(format!("type {:?} has no fields", t.name));
        }
        let mut fields: BTreeSet<&str> = BTreeSet::new();
        for f in &t.fields {
            if !fields.insert(f.name.as_str()) {
                return err(format!(
                    "type {:?} declares the field {:?} twice",
                    t.name, f.name
                ));
            }
            check_ref(
                &format!("{}.{}", t.name, f.name),
                &f.type_ref,
                &types,
                &enums,
            )?;
            for m in &f.requires_members {
                let referenced = referenced_type(&f.type_ref);
                match referenced.and_then(|r| s.types.iter().find(|t| t.name == r)) {
                    // A requires_members entry names a member of the REFERENCED
                    // type, not of this one. Checking it against the wrong type
                    // would let a typo through and emit a validator reading a
                    // field that does not exist.
                    Some(target) => {
                        if !target.fields.iter().any(|tf| &tf.name == m) {
                            return err(format!(
                                "{}.{} requires the member {:?}, which {:?} does not declare",
                                t.name, f.name, m, target.name
                            ));
                        }
                    }
                    None => {
                        return err(format!(
                        "{}.{} declares requires_members but is not a reference to a declared type",
                        t.name, f.name
                    ))
                    }
                }
            }
        }
        for group in &t.exactly_one_of {
            if group.len() < 2 {
                return err(format!(
                    "type {:?} has an exactly-one-of group with {} member(s)",
                    t.name,
                    group.len()
                ));
            }
            for m in group {
                if !fields.contains(m.as_str()) {
                    return err(format!(
                        "type {:?} names {:?} in an exactly-one-of group but has no such field",
                        t.name, m
                    ));
                }
            }
        }
    }
    Ok(s)
}

fn referenced_type(tr: &TypeRef) -> Option<String> {
    if tr.kind == "ref" {
        Some(tr.r#ref.clone())
    } else {
        None
    }
}

fn check_ref(
    where_: &str,
    tr: &TypeRef,
    types: &BTreeSet<&str>,
    enums: &BTreeSet<&str>,
) -> Result<(), SurfaceError> {
    match tr.kind.as_str() {
        "ref" => {
            if !types.contains(tr.r#ref.as_str()) {
                return err(format!(
                    "{where_} references the type {:?}, which the artifact does not define",
                    tr.r#ref
                ));
            }
        }
        "enum" => {
            if !enums.contains(tr.r#enum.as_str()) {
                return err(format!(
                    "{where_} references the enum {:?}, which the artifact does not define",
                    tr.r#enum
                ));
            }
        }
        "array" => match &tr.items {
            Some(items) => return check_ref(&format!("{where_}[]"), items, types, enums),
            None => return err(format!("{where_} is an array with no item type")),
        },
        "map" => match &tr.value {
            Some(value) => return check_ref(&format!("{where_}{{}}"), value, types, enums),
            None => return err(format!("{where_} is a map with no value type")),
        },
        "string" | "bool" | "int" | "object" => {}
        other => return err(format!("{where_} has the unsupported type kind {other:?}")),
    }
    Ok(())
}
