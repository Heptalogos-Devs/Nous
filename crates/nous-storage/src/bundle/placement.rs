use crate::{ReadView, storage_error};
use nous_domain::*;
use sqlx::SqliteConnection;

pub(crate) async fn observation_placement_in(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    observation: &ObservationId,
) -> Result<Option<BundlePlacement>> {
    let value:Option<String>=sqlx::query_scalar("SELECT i.placement_json FROM bundle_resources r JOIN bundle_installations i ON i.id=r.installation_id WHERE i.namespace_id=? AND i.subject_id=? AND r.kind='observation' AND r.local_id=? ORDER BY i.commit_seq DESC LIMIT 1")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(observation.as_str()).fetch_optional(conn).await.map_err(storage_error)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Error::from))
        .transpose()
}
impl ReadView {
    pub async fn bundle_observation_placement(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        observation: &ObservationId,
    ) -> Result<Option<BundlePlacement>> {
        observation_placement_in(&mut self.tx, namespace, subject, observation).await
    }
}
