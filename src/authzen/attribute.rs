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
//! | unknown | never reaches the wire  | refused before the round trip, NOT retryable |
//!
//! Absent and "never mentioned" are the same bytes, and that is correct: both
//! say "there is no such fact". JSON has no way to say "I could not find out",
//! which is precisely why the type has to carry it - the wire cannot.
//!
//! The refusal an unknown attribute produces is NOT retryable, and that is the
//! opposite of what it first looks like. A source that could not answer this
//! second may answer the next one - but that is a statement about a DIFFERENT
//! request. This one carries the unresolved attribute inside it, so resending
//! the identical bytes reproduces the identical refusal forever, and a
//! `while err.retryable()` loop would burn its whole budget on it. Re-resolve
//! the attribute and build a new request; the SDK reports that with its own
//! [`super::AuthZenEvaluationError::Unresolved`] rather than through the
//! server's retryable `evaluation_unavailable`.

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

/// Nesting is not decoration: `context.args.query` and
/// `context.correlation.x-session-id` are LEAVES two levels down, and the
/// refusal for an unresolvable one has to name the leaf. A flat bag whose
/// values were opaque JSON would report `/evaluation/context/correlation` for a
/// single unresolvable session id, which tells an operator to go looking
/// through an object rather than at a member.
///
/// A leaf value, or a nested bag.
///
/// The two cases are NOT public variants, and that is deliberate. The
/// normalisation below holds only if nobody can construct the un-normalised
/// form: with a public `Json(serde_json::Value)` variant a caller could write
/// `AttributeValue::Json(json!({"a": 1}))`, which serialises to the same bytes
/// as the nested form but compares unequal to it - and which
/// [`AttributeMap::validate`] would walk straight past, because an opaque JSON
/// object is not a bag it knows how to descend into. Construct with `From`, read
/// with [`AttributeValue::fold`].
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeValue(AttributeValueInner);

#[derive(Clone, Debug, PartialEq)]
enum AttributeValueInner {
    Json(serde_json::Value),
    Nested(AttributeMap),
}

impl AttributeValue {
    /// Reads whichever case this is.
    ///
    /// The only way to look inside. A caller that wants "the nested bag, if it
    /// is one" has [`AttributeValue::as_nested`]; everything else goes through
    /// here, so a third case added later becomes a compile error rather than a
    /// silently unhandled shape.
    pub fn fold<R>(
        &self,
        on_json: impl FnOnce(&serde_json::Value) -> R,
        on_nested: impl FnOnce(&AttributeMap) -> R,
    ) -> R {
        match &self.0 {
            AttributeValueInner::Json(v) => on_json(v),
            AttributeValueInner::Nested(m) => on_nested(m),
        }
    }

    /// The nested bag, when this is one.
    pub fn as_nested(&self) -> Option<&AttributeMap> {
        match &self.0 {
            AttributeValueInner::Nested(m) => Some(m),
            AttributeValueInner::Json(_) => None,
        }
    }

    /// The leaf JSON, when this is one. Never an object.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match &self.0 {
            AttributeValueInner::Json(v) => Some(v),
            AttributeValueInner::Nested(_) => None,
        }
    }

    /// The nested bag, mutably. Crate-internal: the request builders write
    /// leaves through it, and nothing outside needs to reach in.
    pub(crate) fn as_nested_mut(&mut self) -> Option<&mut AttributeMap> {
        match &mut self.0 {
            AttributeValueInner::Nested(m) => Some(m),
            AttributeValueInner::Json(_) => None,
        }
    }
}

impl From<serde_json::Value> for AttributeValue {
    /// Normalises: a JSON object becomes a NESTED bag whose members are all
    /// `Known`.
    ///
    /// Without the normalisation there would be two representations of the same
    /// bytes - `Json(Value::Object)` and `Nested` - and a round trip through
    /// the wire would silently move a value from one to the other. One
    /// representation means equality on the Rust value means equality on the
    /// wire.
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Object(map) => {
                AttributeValue(AttributeValueInner::Nested(AttributeMap(
                    map.into_iter()
                        .map(|(k, v)| (k, Attribute::Known(AttributeValue::from(v))))
                        .collect(),
                )))
            }
            other => AttributeValue(AttributeValueInner::Json(other)),
        }
    }
}

impl From<&str> for AttributeValue {
    fn from(v: &str) -> Self {
        AttributeValue(AttributeValueInner::Json(serde_json::Value::String(
            v.to_string(),
        )))
    }
}

impl From<String> for AttributeValue {
    fn from(v: String) -> Self {
        AttributeValue(AttributeValueInner::Json(serde_json::Value::String(v)))
    }
}

impl From<bool> for AttributeValue {
    fn from(v: bool) -> Self {
        AttributeValue(AttributeValueInner::Json(serde_json::Value::Bool(v)))
    }
}

impl From<i64> for AttributeValue {
    fn from(v: i64) -> Self {
        AttributeValue(AttributeValueInner::Json(serde_json::Value::Number(
            v.into(),
        )))
    }
}

impl From<AttributeMap> for AttributeValue {
    fn from(v: AttributeMap) -> Self {
        AttributeValue(AttributeValueInner::Nested(v))
    }
}

impl Serialize for AttributeValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.0 {
            AttributeValueInner::Json(v) => v.serialize(s),
            AttributeValueInner::Nested(m) => m.serialize(s),
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

    /// Records one attribute, REPLACING whatever was there.
    ///
    /// This is the map operation, and it behaves like one: a caller writing
    /// here is making a deliberate replacement. If what it replaces was an
    /// unresolved attribute, that fact is gone - which is why the request
    /// builders do not use it. See [`AttributeMap::record`].
    pub fn insert(&mut self, key: impl Into<String>, value: Attribute<AttributeValue>) {
        self.0.insert(key.into(), value);
    }

    /// Records one attribute, DECLINING to overwrite an unresolved one.
    ///
    /// This is the write the request builders use, at every level, and the rule
    /// is uniform: an `Unknown` at `key` survives, and the new value is not
    /// written.
    ///
    /// The rule has to be uniform because the alternative was measured. An
    /// earlier version guarded only the PARENT key - so `with_query` refused to
    /// replace an unresolved `context.args`, and then wrote `query` into it
    /// unguarded. A caller that recorded "nobody could read the request body"
    /// and then wrote a recovered partial query produced a complete-looking
    /// envelope one level down, which is verbatim the scenario the guard was
    /// added to prevent.
    ///
    /// A declined write is not silent: the `Unknown` that survived refuses the
    /// envelope at its own pointer, carrying the reason the caller gave, so the
    /// request is never sent and the caller is told which member and why.
    ///
    /// Returns whether the value was written, for a caller that wants to know.
    pub fn record(&mut self, key: impl Into<String>, value: Attribute<AttributeValue>) -> bool {
        let key = key.into();
        if self.holds_unresolved(&key) {
            return false;
        }
        self.0.insert(key, value);
        true
    }

    /// Whether `key` holds an attribute nobody could resolve.
    ///
    /// ONE place, used by both writes. The rule was duplicated across
    /// [`AttributeMap::record`] and [`AttributeMap::nested_for_write`], and the
    /// duplication was not academic: the two guards read almost identically, so
    /// a mutation aimed at one silently hit the other and a guard nothing was
    /// holding in place looked covered. The sibling Java SDK's gate caught it.
    fn holds_unresolved(&self, key: &str) -> bool {
        matches!(self.0.get(key), Some(Attribute::Unknown(_)))
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

    /// The nested bag at `key`, ready to be written into - unless writing there
    /// would ERASE an unresolved attribute.
    ///
    /// Returns `None` when the key already holds an `Unknown`. That is the one
    /// state a later write must not overwrite: the caller has already said
    /// nobody could resolve this member, and quietly replacing it with a fresh
    /// bag would produce a complete-looking request whose missing fact nothing
    /// records. Declining the write leaves the `Unknown` in place, so
    /// [`AttributeMap::validate`] refuses the envelope at that member and the
    /// request is never sent.
    ///
    /// An `Absent` or a leaf value IS replaced: both are resolved statements,
    /// they carry no unresolvability to lose, and last-write-wins on a map key
    /// is what a caller expects.
    ///
    /// This guards the PARENT only. The leaf write inside the returned bag has
    /// to go through [`AttributeMap::record`], which applies the same rule -
    /// guarding one and not the other is how the first version of this fix left
    /// the defect reachable one level down.
    pub(crate) fn nested_for_write(&mut self, key: &str) -> Option<&mut AttributeMap> {
        if self.holds_unresolved(key) {
            return None;
        }
        match self.0.get(key) {
            Some(Attribute::Known(value)) if value.as_nested().is_some() => {}
            _ => {
                self.insert_known(key, AttributeMap::new());
            }
        }
        match self.0.get_mut(key) {
            Some(Attribute::Known(value)) => value.as_nested_mut(),
            _ => None,
        }
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
                Attribute::Known(value) => {
                    if let Some(nested) = value.as_nested() {
                        nested.validate(&pointer)?;
                    }
                }
                Attribute::Absent => {}
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
