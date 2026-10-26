//! Bounded JSON values retain the numeric representation supplied by serde_json.
use nous_domain::{Error, Result};
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, value::RawValue};
use std::fmt;

pub fn bounded_value(bytes: &[u8], remaining_nodes: &mut usize, max_depth: usize) -> Result<Value> {
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let result = JsonSeed {
        remaining: remaining_nodes,
        depth: max_depth,
    }
    .deserialize(&mut parser)
    .and_then(|value| parser.end().map(|()| value));
    result
        .map_err(|error| Error::invalid("invalid or over-budget JSON document").with_cause(&error))
}
struct JsonSeed<'a> {
    remaining: &'a mut usize,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        if *self.remaining == 0 || self.depth == 0 {
            return Err(de::Error::custom("JSON node/depth limit exceeded"));
        }
        *self.remaining -= 1;
        let raw = <&RawValue>::deserialize(deserializer)?;
        let visitor = JsonVisitor {
            remaining: self.remaining,
            depth: self.depth - 1,
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
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
