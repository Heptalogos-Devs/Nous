// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl MotivationService {
    pub async fn trace_node_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<(MotivationRecord, CognitionTraceSources)> {
        self.require_enabled(subject).await?;
        if reference.domain() != CognitiveDomain::Motivation || !reference.is_cognition_revision() {
            return Err(Error::Invalid(
                "Motivation trace requires an owned exact revision".into(),
            ));
        }
        let record = self
            .read_in(
                tx,
                subject,
                reference.uuid().expect("Motivation UUID"),
                true,
            )
            .await?;
        if record.reference() != *reference {
            return Err(Error::Invalid(
                "Motivation trace reference family mismatch".into(),
            ));
        }
        let mut sources = CognitionTraceSources {
            text_bytes: record.content.text().len(),
            ..Default::default()
        };
        sources.add_basis(record.content.basis());
        sources.add_basis(record.evidence.iter().map(|evidence| &evidence.basis));
        let producer = record.content.producer();
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
