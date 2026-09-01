//! A resolved attribute has THREE states, and `Option` carries two.
//!
//! Every `object` member of the AuthZEN surface - `subject.properties`,
//! `action.properties`, `resource.properties`, `context` - is a bag of facts
//! the CALLER resolved from somewhere else: an identity provider, a trace
//! propagator, a session store. Resolving a fact has three outcomes, and two of
//! them are not the same thing:
//!
//! * **known** - the source answered with a value.
//! * **absent** - the source answered, and the answer is that there is no
//!   value. This user has no department; this batch job has no session. Absent
//!   is ORDINARY RESOLVED DATA. A decision made without it is a complete
//!   decision.
//! * **unknown** - the source could not answer. The identity provider timed
//!   out; the trace header was unreadable. A decision made without an unknown
//!   fact is a decision that MIGHT have gone the other way, reported as
//!   complete.
//!
//! Reaching for `Option<T>` collapses the second and third into `None`, and the
//! collapse always resolves the wrong way: an unknown attribute gets dropped
//! from the request, the server evaluates without it, and the caller is handed
//! a verdict that names every attribute it weighed - including the one nobody
//! could resolve. That is the exact failure the server's own adapter refuses on
//! its side of the wire ("accepting it would report that it was considered when
//! it was not"); this type is the same refusal on the client's side.
//!
//! `Option<Option<T>>` is not the answer either. It is unreadable at a call
//! site, it invites `.flatten()`, and the two `None`s are not distinguished by
//! anything a reader can see.
//!
//! # What each state does to the wire
//!
//! | state   | wire                    | outcome                                     |
//! |---------|-------------------------|---------------------------------------------|
//! | known   | the member, with value  | evaluated                                   |
//! | absent  | the member is OMITTED   | evaluated, without a fact that has no value |
//! | unknown | never reaches the wire  | [`AuthZenError`] before the round trip      |
//!
//! Absent and "never mentioned" are the same bytes, and that is correct: both
//! say "there is no such fact". JSON has no way to say "I could not find out",
//! which is precisely why the type has to carry it - the wire cannot.
//!
//! The refusal an unknown attribute produces is `evaluation_unavailable`, which
//! is the one code in the enumeration that [`AuthZenErrorCode::retryable`]
//! reports as worth retrying. That is not a coincidence: a source that could
//! not answer this second may answer the next one, which is exactly the
//! situation a retry is for.

use super::types_gen::{AuthZenError, AuthZenErrorCode};
use serde::de::{MapAccess, Visitor};
use serde::ser::{Error as _, SerializeMap};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// One attribute, in one of three states.
///
/// Construct with [`Attribute::known`], [`Attribute::absent`] or
/// [`Attribute::unknown`]; read with [`Attribute::fold`], which cannot compile
/// unless the caller has said what each of the three states means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attribute<T> {
    /// The source answered with this value.
    Known(T),
    /// The source answered: there is no value.
    Absent,
    /// The source could not answer. Carries why, which travels into the
    /// refusal so an operator sees the cause and not just the effect.
    Unknown(String),
}

impl<T> Attribute<T> {
    /// The source answered with a value.
    pub fn known(value: impl Into<T>) -> Self {
        Attribute::Known(value.into())
    }

    /// The source answered, and there is no value.
    pub fn absent() -> Self {
        Attribute::Absent
    }

    /// The source could not answer. `why` reaches the refusal message.
    pub fn unknown(why: impl Into<String>) -> Self {
        Attribute::Unknown(why.into())
    }

    /// Reads all three states at once.
    ///
    /// This is the accessor to reach for. It does not compile until the caller
    /// has decided what an unresolvable attribute means for them, which is the
    /// decision `Option` lets you skip.
    pub fn fold<R>(
        &self,
        on_known: impl FnOnce(&T) -> R,
        on_absent: impl FnOnce() -> R,
        on_unknown: impl FnOnce(&str) -> R,
    ) -> R {
        match self {
            Attribute::Known(v) => on_known(v),
            Attribute::Absent => on_absent(),
            Attribute::Unknown(why) => on_unknown(why),
        }
    }

    /// The value, if the source answered with one.
    ///
    /// This DOES collapse absent and unknown into `None`, so it is for
    /// inspection - logging, a debug view - and not for deciding what to send.
    /// Nothing built on it can distinguish "there is no department" from "the
    /// directory was down"; [`Attribute::fold`] can.
    pub fn as_known(&self) -> Option<&T> {
        match self {
            Attribute::Known(v) => Some(v),
            _ => None,
        }
    }

    /// Whether the source answered with a value.
    pub fn is_known(&self) -> bool {
        matches!(self, Attribute::Known(_))
    }

    /// Whether the source answered that there is no value.
    pub fn is_absent(&self) -> bool {
        matches!(self, Attribute::Absent)
    }

    /// Whether the source could not answer.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Attribute::Unknown(_))
    }

    /// Applies `f` to a known value, leaving the other two states alone.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Attribute<U> {
        match self {
            Attribute::Known(v) => Attribute::Known(f(v)),
            Attribute::Absent => Attribute::Absent,
            Attribute::Unknown(why) => Attribute::Unknown(why),
        }
    }
}

/// A leaf value, or a nested bag.
///
/// Nesting is not decoration: `context.args.query` and
/// `context.correlation.x-session-id` are LEAVES two levels down, and the
/// refusal for an unresolvable one has to name the leaf. A flat bag whose
/// values were opaque JSON would report `/evaluation/context/correlation` for a
/// single unresolvable session id, which tells an operator to go looking
/// through an object rather than at a member.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeValue {
    /// A scalar or array. Never an object - see [`AttributeValue::from`].
    Json(serde_json::Value),
    /// A nested bag, whose own members each carry the three states.
    Nested(AttributeMap),
}

impl From<serde_json::Value> for AttributeValue {
    /// Normalises: a JSON object becomes a [`AttributeValue::Nested`] bag whose
    /// members are all `Known`.
    ///
    /// Without the normalisation there would be two representations of the same
    /// bytes - `Json(Value::Object)` and `Nested` - and a round trip through
    /// the wire would silently move a value from one to the other. One
    /// representation means equality on the Rust value means equality on the
    /// wire.
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Object(map) => AttributeValue::Nested(AttributeMap(
                map.into_iter()
                    .map(|(k, v)| (k, Attribute::Known(AttributeValue::from(v))))
                    .collect(),
            )),
            other => AttributeValue::Json(other),
        }
    }
}

impl From<&str> for AttributeValue {
    fn from(v: &str) -> Self {
        AttributeValue::Json(serde_json::Value::String(v.to_string()))
    }
}

impl From<String> for AttributeValue {
    fn from(v: String) -> Self {
        AttributeValue::Json(serde_json::Value::String(v))
    }
}

impl From<bool> for AttributeValue {
    fn from(v: bool) -> Self {
        AttributeValue::Json(serde_json::Value::Bool(v))
    }
}

impl From<i64> for AttributeValue {
    fn from(v: i64) -> Self {
        AttributeValue::Json(serde_json::Value::Number(v.into()))
    }
}

impl From<AttributeMap> for AttributeValue {
    fn from(v: AttributeMap) -> Self {
        AttributeValue::Nested(v)
    }
}

impl Serialize for AttributeValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            AttributeValue::Json(v) => v.serialize(s),
            AttributeValue::Nested(m) => m.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for AttributeValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(AttributeValue::from(serde_json::Value::deserialize(d)?))
    }
}

/// A bag of attributes, ordered by key.
///
/// `BTreeMap` rather than `HashMap` so the same bag always produces the same
/// bytes. An authorization request whose serialisation varies run to run cannot
/// be compared, cached or asserted on, and the difference would surface as a
/// flaky test rather than as a decision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttributeMap(BTreeMap<String, Attribute<AttributeValue>>);

impl AttributeMap {
    /// An empty bag: the caller resolved no attributes at all.
    pub fn new() -> Self {
        AttributeMap(BTreeMap::new())
    }

    /// Whether the bag holds no members.
    ///
    /// An empty bag and an absent bag are the same statement, "no attributes",
    /// which is why the generated types hold an `AttributeMap` rather than an
    /// `Option<AttributeMap>`. A member whose value is `Absent` still counts
    /// here: the caller said something about it, and dropping that distinction
    /// would make a bag of three absent facts indistinguishable from a bag
    /// nobody filled in.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many members the bag holds, in any state.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Records one attribute.
    pub fn insert(&mut self, key: impl Into<String>, value: Attribute<AttributeValue>) {
        self.0.insert(key.into(), value);
    }

    /// Records a resolved value.
    pub fn insert_known(&mut self, key: impl Into<String>, value: impl Into<AttributeValue>) {
        self.insert(key, Attribute::Known(value.into()));
    }

    /// Records that the source answered, and there is no value.
    pub fn insert_absent(&mut self, key: impl Into<String>) {
        self.insert(key, Attribute::Absent);
    }

    /// Records that the source could not answer.
    pub fn insert_unknown(&mut self, key: impl Into<String>, why: impl Into<String>) {
        self.insert(key, Attribute::Unknown(why.into()));
    }

    /// Reads one attribute.
    pub fn get(&self, key: &str) -> Option<&Attribute<AttributeValue>> {
        self.0.get(key)
    }

    /// Iterates the bag in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Attribute<AttributeValue>)> {
        self.0.iter()
    }

    /// Refuses a bag holding an attribute nobody could resolve.
    ///
    /// `at` is the JSON Pointer this bag sits at, so the refusal names the
    /// member the way the server names it - and the FIRST unresolvable member
    /// in key order, so the same bag always produces the same refusal rather
    /// than one that depends on iteration luck.
    ///
    /// The alternative - sending the request without the member - is the
    /// fail-open this type exists to prevent: the server would evaluate a
    /// complete-looking request, the audit row would record a decision made on
    /// the attributes present, and nothing anywhere would record that one of
    /// them was never resolved.
    pub fn validate(&self, at: &str) -> Result<(), AuthZenError> {
        for (key, value) in &self.0 {
            let pointer = format!("{at}/{key}");
            match value {
                Attribute::Unknown(why) => {
                    return Err(AuthZenError::new(
                        AuthZenErrorCode::EvaluationUnavailable,
                        format!(
                            "the attribute {key:?} could not be resolved ({why}); sending the \
                             request without it would obtain a decision that weighed every \
                             attribute except the one nobody could read, and report it as complete"
                        ),
                    )
                    .at(&pointer));
                }
                Attribute::Known(AttributeValue::Nested(nested)) => nested.validate(&pointer)?,
                Attribute::Known(AttributeValue::Json(_)) | Attribute::Absent => {}
            }
        }
        Ok(())
    }
}

impl FromIterator<(String, Attribute<AttributeValue>)> for AttributeMap {
    fn from_iter<I: IntoIterator<Item = (String, Attribute<AttributeValue>)>>(iter: I) -> Self {
        AttributeMap(iter.into_iter().collect())
    }
}

impl Serialize for AttributeMap {
    /// Absent members are omitted; an unknown member is a serialisation
    /// FAILURE.
    ///
    /// [`AttributeMap::validate`] is the check a caller is meant to hit, and it
    /// produces a typed refusal with a pointer. This is the backstop underneath
    /// it: a future code path that encodes an envelope without validating it
    /// first cannot quietly drop the unresolved member, because there is no
    /// encoding of `Unknown` for it to fall back to.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        for (key, value) in &self.0 {
            match value {
                Attribute::Known(v) => map.serialize_entry(key, v)?,
                Attribute::Absent => {}
                Attribute::Unknown(why) => {
                    return Err(S::Error::custom(format!(
                        "the attribute {key:?} could not be resolved ({why}) and has no wire \
                         representation; validate the envelope before encoding it"
                    )))
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AttributeMap {
    /// Every member present on the wire decodes as `Known`.
    ///
    /// There is no decoding that yields `Absent` or `Unknown`, and there should
    /// not be: both are statements about a RESOLUTION the sender performed, and
    /// the wire carries the result of that resolution rather than the
    /// resolution itself.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct BagVisitor;

        impl<'de> Visitor<'de> for BagVisitor {
            type Value = AttributeMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object of attributes")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<AttributeMap, M::Error> {
                let mut out = BTreeMap::new();
                while let Some((k, v)) = access.next_entry::<String, AttributeValue>()? {
                    out.insert(k, Attribute::Known(v));
                }
                Ok(AttributeMap(out))
            }
        }

        d.deserialize_map(BagVisitor)
    }
}
