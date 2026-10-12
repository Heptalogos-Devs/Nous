// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
fn derivation_identity(representation: &DerivedRepresentation) -> Result<(String, String)> {
    let input_digest = blake3::hash(
        &serde_json::to_vec(&representation.inputs).map_err(|e| Error::Invalid(e.to_string()))?,
    )
    .to_hex()
    .to_string();
    let key = blake3::hash(
        &serde_json::to_vec(&(
            representation.subject_id,
            &input_digest,
            representation.representation_kind,
            &representation.producer.signature_hash,
            &representation.strategy,
            representation.supersedes,
        ))
        .map_err(|e| Error::Invalid(e.to_string()))?,
    )
    .to_hex()
    .to_string();
    Ok((input_digest, key))
}
impl MaterialService {
    pub async fn references_producer(&self, subject: SubjectId, producer: Uuid) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM derived_representations WHERE subject_id=$1 AND producer_signature_id=$2)")
            .bind(subject.0)
            .bind(producer)
            .fetch_one(self.store.pool())
            .await
            .map_err(db)
    }

    pub async fn persist_derived_representation(
        &self,
        mut representation: DerivedRepresentation,
        operation: Option<&nous_persistence::WorkflowIdentity>,
    ) -> Result<DerivedRepresentation> {
        if operation.is_some_and(|identity| {
            identity.subject != representation.subject_id || identity.owner != "material"
        }) {
            return Err(Error::Invalid(
                "derivation operation belongs to another owner or Subject".into(),
            ));
        }
        representation.validate()?;
        representation.producer = AuthorityStore::canonical_producer(&representation.producer)?;
        let (input_digest, key) = derivation_identity(&representation)?;
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        self.validate_derivation_inputs_in(&mut tx, &representation)
            .await?;
        representation.revision = if let Some(previous) = representation.supersedes {
            let revision: Option<i32> = sqlx::query_scalar("SELECT revision FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=$2 FOR SHARE").bind(representation.subject_id.0).bind(previous.0).fetch_optional(&mut *tx).await.map_err(db)?;
            revision
                .ok_or_else(|| {
                    Error::Invalid("superseded representation is outside Subject".into())
                })?
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("representation revision overflow".into()))?
        } else {
            1
        };
        if let Some(row) =
            sqlx::query("SELECT * FROM derived_representations WHERE derivation_key=$1")
                .bind(&key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?
        {
            representation.derived_representation_id =
                DerivedRepresentationId(row.try_get("derived_representation_id").map_err(db)?);
            representation.payload_text = row.try_get("payload_text").map_err(db)?;
            representation.payload_json = row.try_get("payload_json").map_err(db)?;
            representation.payload_artifact_id = row
                .try_get::<Option<Uuid>, _>("payload_artifact_id")
                .map_err(db)?
                .map(ArtifactId);
            representation.quality = row.try_get("quality").map_err(db)?;
            representation.created_at = row.try_get("created_at").map_err(db)?;
            representation.revision = row.try_get("revision").map_err(db)?;
            if let Some(identity) = operation {
                AuthorityStore::publish_material_result_in(
                    &mut tx,
                    identity,
                    CognitiveRef::DerivedRepresentation(representation.derived_representation_id),
                )
                .await?;
            }
            tx.commit().await.map_err(db)?;
            return Ok(representation);
        }
        representation.created_at = self.cognition.now(representation.subject_id);
        let producer_id =
            AuthorityStore::register_producer_in(&mut tx, &representation.producer).await?;
        sqlx::query("INSERT INTO derived_representations(derived_representation_id,subject_id,input_digest,strategy,derivation_key,representation_kind,producer_signature_id,revision,payload_text,payload_artifact_id,quality,created_at,supersedes,payload_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
            .bind(representation.derived_representation_id.0).bind(representation.subject_id.0).bind(input_digest).bind(&representation.strategy).bind(key).bind(representation.representation_kind.as_str()).bind(producer_id).bind(representation.revision).bind(&representation.payload_text).bind(representation.payload_artifact_id.map(|id| id.0)).bind(&representation.quality).bind(representation.created_at).bind(representation.supersedes.map(|id| id.0)).bind(&representation.payload_json).execute(&mut *tx).await.map_err(db)?;
        for input in &representation.inputs {
            let (source, derived, region) = match input.reference {
                CognitiveRef::SourceRegion(id) => (Some(id.0), None, None),
                CognitiveRef::DerivedRepresentation(id) => (None, Some(id.0), None),
                CognitiveRef::DerivedRegion(id) => (None, None, Some(id.0)),
                _ => return Err(Error::Invalid("invalid derivation input".into())),
            };
            sqlx::query("INSERT INTO derived_representation_inputs(derived_representation_id,ordinal,role,source_region_id,input_representation_id,derived_region_id) VALUES($1,$2,$3,$4,$5,$6)").bind(representation.derived_representation_id.0).bind(input.ordinal as i32).bind(&input.role).bind(source).bind(derived).bind(region).execute(&mut *tx).await.map_err(db)?;
        }
        sqlx::query("UPDATE coverage_needs SET state='ready',current_representation_id=$2,updated_at=$3 WHERE subject_id=$1 AND source_region_id IN (SELECT source_region_id FROM representation_source_regions($1,$2)) AND representation_kind=$4")
            .bind(representation.subject_id.0).bind(representation.derived_representation_id.0).bind(representation.created_at).bind(representation.representation_kind.as_str()).execute(&mut *tx).await.map_err(db)?;
        AuthorityStore::invalidate_in(
            &mut tx,
            representation.subject_id,
            ProjectionInvalidation::text(),
        )
        .await?;
        self.store
            .ensure_identity_addresses_in(
                &mut tx,
                representation.subject_id,
                &[CognitiveRef::DerivedRepresentation(
                    representation.derived_representation_id,
                )],
                "",
            )
            .await?;
        if let Some(identity) = operation {
            AuthorityStore::publish_material_result_in(
                &mut tx,
                identity,
                CognitiveRef::DerivedRepresentation(representation.derived_representation_id),
            )
            .await?;
        }
        tx.commit().await.map_err(db)?;
        Ok(representation)
    }
    async fn validate_derivation_inputs_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        representation: &DerivedRepresentation,
    ) -> Result<()> {
        // Lock each admitted input relation in bulk; immutable existing edges cannot create a backwards cycle.
        let mut sources = Vec::new();
        let mut derived = Vec::new();
        let mut regions = Vec::new();
        for input in &representation.inputs {
            match input.reference {
                CognitiveRef::SourceRegion(id) => sources.push(id.0),
                CognitiveRef::DerivedRepresentation(id) => derived.push(id.0),
                CognitiveRef::DerivedRegion(id) => regions.push(id.0),
                _ => return Err(Error::Invalid("invalid derivation input kind".into())),
            }
        }
        let locked: i64 = sqlx::query_scalar(r#"
WITH sources AS (SELECT source_region_id FROM source_regions WHERE subject_id=$1 AND source_region_id=ANY($2::uuid[]) FOR SHARE),
representations AS (SELECT derived_representation_id FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=ANY($3::uuid[]) FOR SHARE),
regions AS (SELECT derived_region_id FROM derived_regions WHERE subject_id=$1 AND derived_region_id=ANY($4::uuid[]) FOR SHARE),
payload AS (SELECT artifact_id FROM artifacts WHERE subject_id=$1 AND artifact_id=$5 FOR SHARE)
SELECT (SELECT count(*) FROM sources)+(SELECT count(*) FROM representations)+(SELECT count(*) FROM regions)+(SELECT count(*) FROM payload)
"#).bind(representation.subject_id.0).bind(&sources).bind(&derived).bind(&regions)
            .bind(representation.payload_artifact_id.map(|id|id.0)).fetch_one(&mut **tx).await.map_err(db)?;
        if locked as usize
            != representation.inputs.len()
                + usize::from(representation.payload_artifact_id.is_some())
        {
            return Err(Error::Invalid(
                "derivation input or payload is missing or outside Subject".into(),
            ));
        }
        self.validate_field_basis(tx, representation).await?;
        Ok(())
    }

    /// Callers establish parent ownership once; segmentation already holds the parent row.
    pub(super) async fn insert_derived_regions_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        regions: &[DerivedRegion],
    ) -> Result<Vec<DerivedRegion>> {
        if regions.is_empty() {
            return Ok(Vec::new());
        }
        let subject = regions[0].subject_id;
        let requested =
            serde_json::to_value(regions).map_err(|error| Error::Invalid(error.to_string()))?;
        let written = sqlx::query(r#"
INSERT INTO derived_regions(derived_region_id,subject_id,derived_representation_id,coordinate_kind,coordinate,coordinate_hash,parent_derived_region_id,created_at)
SELECT derived_region_id,subject_id,derived_representation_id,coordinate_kind,coordinate,coordinate_hash,parent_derived_region_id,$2
FROM jsonb_to_recordset($1) r(derived_region_id uuid,subject_id uuid,derived_representation_id uuid,coordinate_kind text,coordinate jsonb,coordinate_hash text,parent_derived_region_id uuid)
ON CONFLICT(derived_representation_id,coordinate_kind,coordinate_hash) DO NOTHING
"#).bind(&requested).bind(self.cognition.now(subject)).execute(&mut **tx).await.map_err(db)?.rows_affected();
        let values: Vec<serde_json::Value> = sqlx::query_scalar(r#"
SELECT to_jsonb(d) FROM jsonb_array_elements($1) WITH ORDINALITY AS input(value,ordinal)
CROSS JOIN LATERAL jsonb_to_record(input.value) AS r(derived_representation_id uuid,coordinate_kind text,coordinate_hash text)
JOIN derived_regions d USING(derived_representation_id,coordinate_kind,coordinate_hash) ORDER BY input.ordinal
"#).bind(requested).fetch_all(&mut **tx).await.map_err(db)?;
        let regions = values
            .into_iter()
            .map(|value| {
                serde_json::from_value::<DerivedRegion>(value)
                    .map_err(|error| Error::Infrastructure(error.to_string()))
            })
            .collect::<Result<Vec<_>>>()?;
        self.store
            .ensure_identity_addresses_in(
                tx,
                subject,
                &regions
                    .iter()
                    .map(|region| CognitiveRef::DerivedRegion(region.derived_region_id))
                    .collect::<Vec<_>>(),
                "",
            )
            .await?;
        if written > 0 {
            AuthorityStore::invalidate_in(tx, subject, ProjectionInvalidation::text()).await?;
        }
        Ok(regions)
    }
}
