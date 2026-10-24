//! Typed source catalogs, provider proposals, and explicit group compilation.

mod compile;
mod dto;
mod input;
mod runtime;
mod schema;
pub use dto::*;
pub use input::{SourceCatalogEntry, StructuredFormationInput};
pub use runtime::{PreparedStructured, definitions, execute, prepare, role};
pub use schema::structured_output_schema;
const MAX_GROUPS: usize = 8;
const MAX_CHANGES: usize = 32;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
