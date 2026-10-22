//! Declared planner parameters.
//!
//! A planner declares every parameter it accepts (`ParamSpec`). Values a user
//! gives (`ParamValues`) are checked against those declarations by `resolve`,
//! which fills in defaults and rejects unknown keys and out-of-range values, so
//! no value is ever silently replaced. `ResolvedParams::canonical` is the text
//! form recorded with results and used in run identities.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Type and bounds of a declared parameter. Bounds are inclusive.
#[derive(Debug, Clone, Copy)]
pub enum ParamKind {
    Int { min: i64, max: i64, default: i64 },
}

/// One parameter a planner accepts.
#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: ParamKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
}

impl fmt::Display for ParamValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamValue::Int(v) => write!(f, "{v}"),
        }
    }
}

/// Parameter values as a user gave them, not yet checked against any planner.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParamValues(BTreeMap<String, ParamValue>);

impl ParamValues {
    pub fn with(mut self, key: &str, value: ParamValue) -> Self {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: ParamValue) {
        self.0.insert(key.to_string(), value);
    }

    pub fn get(&self, key: &str) -> Option<ParamValue> {
        self.0.get(key).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Every declared parameter of one planner, each with a checked value.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedParams(BTreeMap<&'static str, ParamValue>);

impl ResolvedParams {
    /// Value of a declared integer parameter. Asking for a key the planner
    /// never declared is a programming error.
    pub fn int(&self, key: &str) -> i64 {
        match self.0.get(key) {
            Some(ParamValue::Int(v)) => *v,
            None => panic!("parameter '{key}' is not declared by this planner"),
        }
    }

    /// `key=value` pairs sorted by key and joined by `,`; empty when the
    /// planner declares nothing.
    pub fn canonical(&self) -> String {
        self.0.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(",")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamError {
    Unknown { key: String },
    OutOfRange { key: String, value: i64, min: i64, max: i64 },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamError::Unknown { key } => write!(f, "unknown parameter '{key}'"),
            ParamError::OutOfRange { key, value, min, max } => {
                write!(f, "parameter '{key}' = {value} is outside {min}..={max}")
            }
        }
    }
}

/// Checks `values` against `specs`: every given key must be declared and in
/// range; every declared key missing from `values` takes its default.
pub fn resolve(specs: &[ParamSpec], values: &ParamValues) -> Result<ResolvedParams, ParamError> {
    if let Some(key) =
        values.0.keys().find(|key| !specs.iter().any(|spec| spec.key == key.as_str()))
    {
        return Err(ParamError::Unknown { key: key.clone() });
    }
    let mut resolved = BTreeMap::new();
    for spec in specs {
        let value = match spec.kind {
            ParamKind::Int { min, max, default } => {
                let ParamValue::Int(value) =
                    values.get(spec.key).unwrap_or(ParamValue::Int(default));
                if !(min..=max).contains(&value) {
                    return Err(ParamError::OutOfRange {
                        key: spec.key.to_string(),
                        value,
                        min,
                        max,
                    });
                }
                ParamValue::Int(value)
            }
        };
        resolved.insert(spec.key, value);
    }
    Ok(ResolvedParams(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[ParamSpec] = &[
        ParamSpec {
            key: "b",
            label: "B",
            help: "",
            kind: ParamKind::Int { min: 0, max: 3, default: 2 },
        },
        ParamSpec {
            key: "a",
            label: "A",
            help: "",
            kind: ParamKind::Int { min: 1, max: 9, default: 5 },
        },
    ];

    fn given(key: &str, value: i64) -> ParamValues {
        ParamValues::default().with(key, ParamValue::Int(value))
    }

    #[test]
    fn defaults_fill_missing_keys() {
        let params = resolve(SPECS, &ParamValues::default()).unwrap();
        assert_eq!((params.int("a"), params.int("b")), (5, 2));
    }

    #[test]
    fn given_value_replaces_default() {
        assert_eq!(resolve(SPECS, &given("a", 7)).unwrap().int("a"), 7);
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert_eq!(resolve(SPECS, &given("z", 1)), Err(ParamError::Unknown { key: "z".into() }));
    }

    #[test]
    fn bounds_are_inclusive() {
        assert!(resolve(SPECS, &given("a", 1)).is_ok());
        assert!(resolve(SPECS, &given("a", 9)).is_ok());
    }

    #[test]
    fn outside_bounds_is_an_error() {
        for value in [0, 10] {
            assert_eq!(
                resolve(SPECS, &given("a", value)),
                Err(ParamError::OutOfRange { key: "a".into(), value, min: 1, max: 9 })
            );
        }
    }

    #[test]
    fn canonical_text_sorts_keys() {
        assert_eq!(resolve(SPECS, &ParamValues::default()).unwrap().canonical(), "a=5,b=2");
    }

    #[test]
    fn nothing_declared_gives_empty_text() {
        assert_eq!(resolve(&[], &ParamValues::default()).unwrap().canonical(), "");
    }

    #[test]
    fn json_round_trip() {
        let values = given("a", 7);
        let json = serde_json::to_string(&values).unwrap();
        assert_eq!(json, r#"{"a":7}"#);
        assert_eq!(serde_json::from_str::<ParamValues>(&json).unwrap(), values);
    }
}
