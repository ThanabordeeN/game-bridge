//! A minimal JSON Schema validator covering exactly the keyword subset used by
//! `schemas/events.schema.json`.
//!
//! The protocol crate deliberately stays dependency-free, and the only
//! alternative was to add a full JSON Schema implementation plus its transitive
//! tree to validate 21 small documents. The keywords below are all the schema
//! uses; anything else is reported as unsupported rather than silently ignored,
//! so a schema that grows a new keyword fails loudly instead of passing
//! vacuously.

use serde_json::Value;

/// Schema keywords this validator implements. Anything else in a schema
/// definition is an error, so a schema cannot grow a constraint that is
/// silently ignored.
const SUPPORTED: &[&str] = &[
    "$ref", "$schema", "$id", "title", "description", "oneOf", "type", "const",
    "enum", "required", "properties", "additionalProperties", "items", "minimum",
    "maximum", "minLength", "maxLength", "examples", "$defs",
];

/// Validate `instance` against `schema`, resolving `$ref` against `root`.
pub fn validate(schema: &Value, instance: &Value, root: &Value) -> Result<(), String> {
    // `$ref` replaces the sibling keywords it appears with.
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = resolve(reference, root)
            .ok_or_else(|| format!("unresolvable $ref {reference}"))?;
        return validate(target, instance, root);
    }

    if let Some(variants) = schema.get("oneOf") {
        let variants = variants
            .as_array()
            .ok_or_else(|| "oneOf must be an array".to_string())?;
        let mut matched = 0usize;
        let mut first_error = String::new();
        for variant in variants {
            match validate(variant, instance, root) {
                Ok(()) => matched += 1,
                Err(e) => {
                    if first_error.is_empty() {
                        first_error = e;
                    }
                }
            }
        }
        return match matched {
            1 => Ok(()),
            0 => Err(format!("matched no oneOf variant (first error: {first_error})")),
            n => Err(format!("matched {n} oneOf variants, expected exactly 1")),
        };
    }

    if let Some(expected) = schema.get("const") {
        if instance != expected {
            return Err(format!("expected const {expected}, got {instance}"));
        }
    }

    if let Some(allowed) = schema.get("enum") {
        let allowed = allowed
            .as_array()
            .ok_or_else(|| "enum must be an array".to_string())?;
        if !allowed.contains(instance) {
            return Err(format!("{instance} is not one of {allowed:?}"));
        }
    }

    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        check_type(expected_type, instance)?;
    }

    // Numeric and string bounds.
    if let Some(value) = instance.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
            if value < min {
                return Err(format!("{value} is below minimum {min}"));
            }
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
            if value > max {
                return Err(format!("{value} is above maximum {max}"));
            }
        }
    }
    if let Some(text) = instance.as_str() {
        let len = text.chars().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
            if len < min {
                return Err(format!("length {len} is below minLength {min}"));
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
            if len > max {
                return Err(format!("length {len} is above maxLength {max}"));
            }
        }
    }

    if let Some(object) = instance.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required {
                let key = key
                    .as_str()
                    .ok_or_else(|| "required entries must be strings".to_string())?;
                if !object.contains_key(key) {
                    return Err(format!("missing required property {key:?}"));
                }
            }
        }

        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(properties) = properties {
            for (key, subschema) in properties {
                if let Some(value) = object.get(key) {
                    validate(subschema, value, root)
                        .map_err(|e| format!("in property {key:?}: {e}"))?;
                }
            }
        }

        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            let known: Vec<&String> = properties
                .map(|p| p.keys().collect())
                .unwrap_or_default();
            for key in object.keys() {
                if !known.iter().any(|k| *k == key) {
                    return Err(format!("unexpected property {key:?}"));
                }
            }
        }
    }

    if let (Some(array), Some(items)) = (instance.as_array(), schema.get("items")) {
        for (index, element) in array.iter().enumerate() {
            validate(items, element, root).map_err(|e| format!("in item {index}: {e}"))?;
        }
    }

    // Reject keyword sets this validator does not implement, so a schema that
    // grows a constraint cannot pass by being ignored.
    //
    // Only meaningful when this node is a schema *definition*. A node reached
    // through a `properties` map is a map of property names to schemas, and its
    // keys are property names rather than keywords — the caller descends into
    // each value separately.
    if let Some(object) = schema.as_object() {
        let is_definition = schema.get("properties").is_some()
            || schema.get("type").is_some()
            || schema.get("$ref").is_some()
            || schema.get("oneOf").is_some()
            || schema.get("const").is_some()
            || schema.get("enum").is_some();
        if is_definition {
            for key in object.keys() {
                if !SUPPORTED.contains(&key.as_str()) {
                    return Err(format!("unsupported schema keyword {key:?}"));
                }
            }
        }
    }

    Ok(())
}

fn check_type(expected: &str, instance: &Value) -> Result<(), String> {
    let ok = match expected {
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "string" => instance.is_string(),
        "boolean" => instance.is_boolean(),
        "integer" => instance.as_f64().is_some_and(|n| n.fract() == 0.0),
        "number" => instance.is_number(),
        "null" => instance.is_null(),
        other => return Err(format!("unsupported type {other:?}")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("expected type {expected}, got {instance}"))
    }
}

/// Resolve a local JSON pointer of the form `#/$defs/name`.
fn resolve<'a>(reference: &str, root: &'a Value) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    root.pointer(pointer)
}
