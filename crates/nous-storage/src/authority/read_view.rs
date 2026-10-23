use crate::{Database, storage_error};
use nous_domain::Result;
use sqlx::{Sqlite, Transaction};

/// A repository-bound WAL snapshot. Connections stay inside storage.
pub struct ReadView {
    pub(crate) tx: Transaction<'static, Sqlite>,
    pub watermark: i64,
}
impl Database {
    pub async fn read_view(&self) -> Result<ReadView> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let watermark =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        Ok(ReadView { tx, watermark })
    }
}
impl ReadView {
    pub async fn finish(self) -> Result<()> {
        self.tx.commit().await.map_err(storage_error)
    }
}
