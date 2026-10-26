mod call_content;
mod media_snapshot;
mod media_source;
mod records;
mod redaction;
mod retirement;
mod runs;
mod snapshot;
mod subscriptions;
pub use call_content::{EmbeddingCallContent, embedding_call_contents};
pub(crate) mod source;
pub use media_snapshot::{MediaRetrievalEntry, MediaRetrievalSnapshot};
use nous_domain::*;
pub use runs::EmbeddingClaim;
pub use snapshot::{CognitiveRetrievalSnapshot, MemoryRetrievalEntry, StructuredRetrievalEntry};
use sqlx::{Row, sqlite::SqliteRow};
pub(crate) fn target_row(row: &SqliteRow) -> Result<EmbeddingTarget> {
    serde_json::from_str(row.get("target")).map_err(Error::from)
}
