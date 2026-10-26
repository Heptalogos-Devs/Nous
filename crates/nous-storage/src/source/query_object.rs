use crate::{ReadView, storage_error};
use nous_domain::*;
use sqlx::Row;
use std::collections::BTreeSet;
impl ReadView {
    pub async fn query_external_object_targets(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        external: &ExternalObjectRef,
        as_of: Option<i64>,
        history: bool,
    ) -> Result<BTreeSet<RetrievalTarget>> {
        external.validate()?;
        self.subject(namespace, subject).await?;
        let rows=sqlx::query("SELECT id,material_id FROM observations WHERE namespace_id=? AND subject_id=? AND json_extract(origin,'$.external_object_ref')=? AND (? IS NULL OR commit_seq<=?)")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(external.as_str()).bind(as_of).bind(as_of).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let observations = rows
            .iter()
            .map(|row| ObservationId(row.get("id")))
            .collect::<BTreeSet<_>>();
        let mut targets = rows
            .iter()
            .map(|row| {
                RetrievalTarget::Media(media::MediaReference::Original {
                    material_id: MaterialId(row.get("material_id")),
                })
            })
            .collect::<BTreeSet<_>>();
        if observations.is_empty() {
            return Ok(targets);
        }
        for reference in
            crate::vector::query_references(&mut self.tx, namespace, subject, as_of, history)
                .await?
        {
            let closure = super::closure::closure_at(
                &mut self.tx,
                namespace,
                subject,
                reference.clone(),
                as_of,
            )
            .await?;
            if closure.nodes.iter().any(|node|matches!(&node.source,EvidenceSource::Observation{observation_id}if observations.contains(observation_id))){targets.insert(RetrievalTarget::Cognition(reference));}
        }
        Ok(targets)
    }
}
