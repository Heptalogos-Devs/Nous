//! Retained media revisions own artifact bytes independently of task state.
use crate::{ReadView, WriteTransaction, storage_error};
use nous_domain::{Result, media::BlobReference};
use sqlx::SqliteConnection;

macro_rules! artifact_references {
    ($owner:ty) => {
        impl $owner {
            /// Include all revisions: history and unfinished derivations can
            /// still reference an earlier artifact after its head advances.
            pub async fn media_artifact_references(&mut self) -> Result<Vec<BlobReference>> {
                retained_artifacts(&mut self.tx).await
            }
        }
    };
}
artifact_references!(ReadView);
artifact_references!(WriteTransaction);

async fn retained_artifacts(connection: &mut SqliteConnection) -> Result<Vec<BlobReference>> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT json_extract(body,'$.payload.blob') FROM media_representations WHERE json_extract(body,'$.payload.kind')='artifact' ORDER BY 1",
    )
    .fetch_all(connection)
    .await
    .map_err(storage_error)?;
    rows.into_iter()
        .map(|body| {
            let reference: BlobReference = serde_json::from_str(&body)?;
            reference.validate()?;
            Ok(reference)
        })
        .collect()
}
