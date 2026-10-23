mod records;
mod retirement;
mod runs;
mod snapshot;
pub(crate) mod source;
use crate::codec::*;
use nous_domain::*;
pub use runs::EmbeddingClaim;
pub use snapshot::{
    CognitiveRetrievalSnapshot, MemoryRetrievalEntry, RootMaterial, StructuredRetrievalEntry,
};
use sqlx::{Row, sqlite::SqliteRow};
pub(crate) fn target_kind(target: &EmbeddingTarget) -> &'static str {
    match target {
        EmbeddingTarget::Cognition(_) => "cognition",
        EmbeddingTarget::Tag => "tag",
        EmbeddingTarget::Query => "query",
    }
}
pub(crate) fn target_row(row: &SqliteRow) -> Result<EmbeddingTarget> {
    Ok(match row.get::<&str, _>("target_kind") {
        "cognition" => EmbeddingTarget::Cognition(CognitiveRef {
            kind: parse_enum(row.get("cognition_kind"))?,
            id: row.get("cognition_id"),
            revision: row.get("cognition_revision"),
        }),
        "tag" => EmbeddingTarget::Tag,
        "query" => EmbeddingTarget::Query,
        _ => return Err(Error::internal("invalid persisted embedding target")),
    })
}
