mod media_snapshot;
mod media_source;
mod records;
mod retirement;
mod runs;
mod snapshot;
pub(crate) mod source;
pub use media_snapshot::{MediaRetrievalEntry, MediaRetrievalSnapshot};
use nous_domain::*;
pub use runs::EmbeddingClaim;
pub use snapshot::{CognitiveRetrievalSnapshot, MemoryRetrievalEntry, StructuredRetrievalEntry};
use sqlx::{Row, sqlite::SqliteRow};
pub(crate) fn target_row(row: &SqliteRow) -> Result<EmbeddingTarget> {
    serde_json::from_str(row.get("target")).map_err(Error::from)
}
