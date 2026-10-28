mod call_content;
mod inventory;
mod media_snapshot;
mod media_source;
mod package;
mod records;
mod redaction;
mod restoration;
mod retirement;
mod runs;
mod snapshot;
mod subscriptions;
pub use call_content::{EmbeddingCallContent, embedding_call_contents};
pub use inventory::QuerySnapshotOptions;
pub(crate) use inventory::references as query_references;
pub(crate) use runs::space as embedding_space_in;
pub(crate) mod source;
pub use media_snapshot::{MediaRetrievalEntry, MediaRetrievalSnapshot};
use nous_domain::*;
pub use runs::EmbeddingClaim;
pub use snapshot::{CognitiveRetrievalSnapshot, MemoryRetrievalEntry, StructuredRetrievalEntry};
use sqlx::{Row, sqlite::SqliteRow};
pub(crate) fn target_row(row: &SqliteRow) -> Result<EmbeddingTarget> {
    serde_json::from_str(row.get("target")).map_err(Error::from)
}
