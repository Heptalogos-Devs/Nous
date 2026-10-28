//! Bounded JSON values retain the numeric representation supplied by serde_json.
use nous_domain::{Error, ErrorCode, Result};
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, value::RawValue};
use std::fmt;

pub fn bounded_value(bytes: &[u8], remaining_nodes: &mut usize, max_depth: usize) -> Result<Value> {
    let available_nodes = *remaining_nodes;
    let mut exceeded = None;
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let result = JsonSeed {
        remaining: remaining_nodes,
        depth: max_depth,
        exceeded: &mut exceeded,
    }
    .deserialize(&mut parser)
    .and_then(|value| parser.end().map(|()| value));
    result.map_err(|error| {
        let failure = match exceeded {
            Some(JsonLimit::Nodes) => {
                Error::new(ErrorCode::ResourceExhausted, "JSON node budget exhausted")
                    .with_context("resource", "json_nodes")
                    .with_context("available_nodes", available_nodes)
            }
            Some(JsonLimit::Depth) => Error::new(
                ErrorCode::ResourceExhausted,
                "JSON nesting depth exceeds configured limit",
            )
            .with_context("resource", "json_depth")
            .with_context("limit", max_depth),
            None => Error::invalid("invalid JSON document"),
        };
        failure.with_cause(&error)
    })
}
enum JsonLimit {
    Nodes,
    Depth,
}
struct JsonSeed<'a> {
    remaining: &'a mut usize,
    depth: usize,
    exceeded: &'a mut Option<JsonLimit>,
}
impl<'de> DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth == 0 {
            *self.exceeded = Some(JsonLimit::Depth);
            return Err(de::Error::custom("JSON depth limit exceeded"));
        }
        if *self.remaining == 0 {
            *self.exceeded = Some(JsonLimit::Nodes);
            return Err(de::Error::custom("JSON node limit exceeded"));
        }
        *self.remaining -= 1;
        let raw = <&RawValue>::deserialize(deserializer)?;
        let visitor = JsonVisitor {
            remaining: self.remaining,
            depth: self.depth - 1,
            exceeded: self.exceeded,
        };
        // The raw token is part of the public serde_json interface. Numeric
        // leaves go through Value's own precision-aware deserializer.
        let mut parser = serde_json::Deserializer::from_str(raw.get());
        let result = match raw.get().trim_start().as_bytes().first() {
            Some(b'{') => de::Deserializer::deserialize_map(&mut parser, visitor),
            Some(b'[') => de::Deserializer::deserialize_seq(&mut parser, visitor),
            _ => Value::deserialize(&mut parser),
        };
        result
            .and_then(|value| parser.end().map(|()| value))
            .map_err(de::Error::custom)
    }
}
struct JsonVisitor<'a> {
    remaining: &'a mut usize,
    depth: usize,
    exceeded: &'a mut Option<JsonLimit>,
}
impl<'de> Visitor<'de> for JsonVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("bounded JSON object or array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonSeed {
            remaining: self.remaining,
            depth: self.depth,
            exceeded: self.exceeded,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> std::result::Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = object.next_value_seed(JsonSeed {
                remaining: self.remaining,
                depth: self.depth,
                exceeded: self.exceeded,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
