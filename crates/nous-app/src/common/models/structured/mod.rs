//! Typed source catalogs, provider proposals, and explicit group compilation.
//! Adapted source commits, SHA-256 and licenses: ../structured-sources.json.
mod compile;
mod dto;
mod input;
mod runtime;
mod schema;
pub use dto::*;
pub use input::{SourceCatalogEntry, StructuredFormationInput};
pub use runtime::{
    PreparedStructured, StructuredCallMetadata, StructuredCallRecord, StructuredExecution,
    StructuredFormationConfig,
};
pub use schema::structured_output_schema;
const MAX_GROUPS: usize = 8;
const MAX_CHANGES: usize = 32;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
