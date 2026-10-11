// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::ConfigDescriptor;
use serde_json::Value;

/// Configuration annotations describe the catalog, not execution constraints.
/// Only schema positions are traversed: a property named `description` remains a property.
fn schema_identity(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        for key in ["title", "description", "examples", "$comment"] {
            object.remove(key);
        }
        for key in [
            "$defs",
            "definitions",
            "properties",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(children) = object.get_mut(key).and_then(Value::as_object_mut) {
                for child in children.values_mut() {
                    *child = schema_identity(child.take());
                }
            }
        }
        for key in [
            "additionalProperties",
            "unevaluatedProperties",
            "items",
            "contains",
            "propertyNames",
            "not",
            "if",
            "then",
            "else",
            "contentSchema",
        ] {
            if let Some(child) = object.get_mut(key) {
                *child = schema_identity(child.take());
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(children) = object.get_mut(key).and_then(Value::as_array_mut) {
                for child in children {
                    *child = schema_identity(child.take());
                }
            }
        }
    }
    value
}

pub(crate) fn descriptor_identity(descriptor: &ConfigDescriptor) -> Value {
    let mut value = serde_json::to_value(descriptor).expect("descriptor JSON");
    let object = value.as_object_mut().expect("descriptor object");
    for key in [
        "title",
        "description",
        "category",
        "exposure",
        "sensitivity",
    ] {
        object.remove(key);
    }
    object.insert(
        "json_schema".into(),
        schema_identity(descriptor.json_schema.clone()),
    );
    value
}
