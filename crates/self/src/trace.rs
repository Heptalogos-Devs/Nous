// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SelfService {
    pub async fn trace_node_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<(SelfRecord, CognitionTraceSources)> {
        self.require_enabled(subject).await?;
        if reference.domain() != CognitiveDomain::SelfCognition
            || !reference.is_cognition_revision()
        {
            return Err(Error::Invalid(
                "Self trace requires an owned exact revision".into(),
            ));
        }
        let record = self
            .read_in(tx, subject, reference.uuid().expect("Self UUID"), true)
            .await?;
        if record.reference() != *reference {
            return Err(Error::Invalid(
                "Self trace reference family mismatch".into(),
            ));
        }
        let mut sources = CognitionTraceSources {
            text_bytes: record.content.text().len(),
            ..Default::default()
        };
        sources.add_basis(record.content.basis());
        sources.add_basis(record.evidence.iter().map(|evidence| &evidence.basis));
        let producer = match &record.content {
            SelfContent::SelfFacet(facet) => {
                for exception in &facet.exceptions {
                    sources.add_basis(&exception.basis);
                }
                facet.producer.as_ref()
            }
            SelfContent::NarrativeThread(narrative) => {
                sources.references.extend(narrative.source_refs.clone());
                narrative.producer.as_ref()
            }
        };
        if let Some(producer) = producer {
            let id = sqlx::query_scalar(
                "SELECT producer_signature_id FROM producer_signatures WHERE signature_hash=$1",
            )
            .bind(&producer.signature_hash)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
            sources.producers.push(id);
        }
        Ok((record, sources))
    }
}

impl SelfService {
    /// Dependency metadata only; no content, evidence prose or producer body is read.
    pub async fn source_references_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<Vec<CognitiveRef>> {
        if reference.domain() != CognitiveDomain::SelfCognition
            || !reference.is_cognition_revision()
        {
            return Err(Error::Invalid(
                "Self source metadata requires exact revision".into(),
            ));
        }
        let rows: Vec<(String,String)> = sqlx::query_as("SELECT d.ref_kind,d.ref_value FROM self_dependencies d JOIN self_revisions r USING(revision_id) WHERE r.subject_id=$1 AND r.revision_id=$2 ORDER BY d.ref_kind,d.ref_value")
            .bind(subject.0).bind(reference.uuid()).fetch_all(&mut **tx).await.map_err(db)?;
        rows.into_iter()
            .map(|(k, v)| parse_reference(&k, &v))
            .collect()
    }
}
