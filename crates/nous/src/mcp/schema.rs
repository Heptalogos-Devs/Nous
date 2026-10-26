use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::sync::Arc;

pub(super) fn object_input<T: JsonSchema + 'static>() -> Arc<Map<String, Value>> {
    let mut schema = rmcp::handler::server::common::schema_for_type::<T>().as_ref().clone();
    schema.insert("type".into(), Value::String("object".into()));
    schema.remove("title");
    schema.remove("description");
    Arc::new(schema)
}
