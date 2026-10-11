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
    ) -> Result<DerivedRepresentation> {
        representation.validate()?;
        representation.producer = AuthorityStore::canonical_producer(&representation.producer)?;
        let (input_digest, key) = derivation_identity(&representation)?;
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&key)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        // All inputs must already exist. Together with immutable edges and a new
        // representation identity, this admits no backwards edge or DAG cycle.
        for input in &representation.inputs {
            let (query, id) = match input.reference {
                CognitiveRef::SourceRegion(id) => (
                    "SELECT subject_id FROM source_regions WHERE source_region_id=$1 FOR SHARE",
                    id.0,
                ),
                CognitiveRef::DerivedRepresentation(id) => (
                    "SELECT subject_id FROM derived_representations WHERE derived_representation_id=$1 FOR SHARE",
                    id.0,
                ),
                CognitiveRef::DerivedRegion(id) => (
                    "SELECT subject_id FROM derived_regions WHERE derived_region_id=$1 FOR SHARE",
                    id.0,
                ),
                _ => return Err(Error::Invalid("invalid derivation input kind".into())),
            };
            let owner: Option<Uuid> = sqlx::query_scalar(query)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db)?;
            if owner != Some(representation.subject_id.0) {
                return Err(Error::Invalid(
                    "derivation input is missing or outside Subject".into(),
                ));
            }
        }
        self.validate_field_basis(&mut tx, &representation).await?;
        if let Some(artifact) = representation.payload_artifact_id {
            let owner: Option<Uuid> = sqlx::query_scalar(
                "SELECT subject_id FROM artifacts WHERE artifact_id=$1 FOR SHARE",
            )
            .bind(artifact.0)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if owner != Some(representation.subject_id.0) {
                return Err(Error::Invalid(
                    "derived payload artifact is outside Subject".into(),
                ));
            }
        }
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
        tx.commit().await.map_err(db)?;
        Ok(representation)
    }
    pub async fn persist_derived_region(&self, mut region: DerivedRegion) -> Result<DerivedRegion> {
        region.validate()?;
        let mut tx = self.store.begin().await?;
        region.derived_region_id = self.insert_derived_region_in_tx(&mut tx, &region).await?;
        region.created_at = sqlx::query_scalar(
            "SELECT created_at FROM derived_regions WHERE derived_region_id=$1 AND subject_id=$2",
        )
        .bind(region.derived_region_id.0)
        .bind(region.subject_id.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        nous_persistence::AuthorityStore::invalidate_in(
            &mut tx,
            region.subject_id,
            ProjectionInvalidation::text(),
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(region)
    }

    pub(super) async fn insert_derived_region_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        region: &DerivedRegion,
    ) -> Result<DerivedRegionId> {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=$2)")
            .bind(region.subject_id.0)
            .bind(region.derived_representation_id.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
        if !valid
            || region.coordinate_kind.trim().is_empty()
            || region.coordinate_hash.trim().is_empty()
        {
            return Err(Error::Invalid(
                "derived region is invalid or outside Subject".into(),
            ));
        }
        if let Some(parent) = region.parent_derived_region_id {
            let parent_valid: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM derived_regions WHERE subject_id=$1 AND derived_region_id=$2 AND derived_representation_id=$3)",
            )
            .bind(region.subject_id.0)
            .bind(parent.0)
            .bind(region.derived_representation_id.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
            if !parent_valid {
                return Err(Error::Invalid(
                    "derived region parent is outside the representation".into(),
                ));
            }
        }
        let actual_id: Uuid = sqlx::query_scalar("INSERT INTO derived_regions(derived_region_id,subject_id,derived_representation_id,coordinate_kind,coordinate,coordinate_hash,parent_derived_region_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(derived_representation_id,coordinate_kind,coordinate_hash) DO UPDATE SET coordinate_hash=excluded.coordinate_hash RETURNING derived_region_id")
            .bind(region.derived_region_id.0)
            .bind(region.subject_id.0)
            .bind(region.derived_representation_id.0)
            .bind(&region.coordinate_kind)
            .bind(&region.coordinate)
            .bind(&region.coordinate_hash)
            .bind(region.parent_derived_region_id.map(|id| id.0))
            .bind(self.cognition.now(region.subject_id))
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
        let actual_id = DerivedRegionId(actual_id);
        self.store
            .ensure_identity_addresses_in(
                tx,
                region.subject_id,
                &[CognitiveRef::DerivedRegion(actual_id)],
                "",
            )
            .await?;
        Ok(actual_id)
    }
}
