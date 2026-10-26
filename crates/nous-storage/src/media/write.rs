use super::{graph, read};
use crate::{WriteTransaction, codec::*, storage_error, write_error};
use nous_domain::{media::*, *};
use sha2::{Digest, Sha256};
use sqlx::Row;
impl WriteTransaction {
    pub async fn put_source_region(&mut self, mut region: SourceRegion) -> Result<SourceRegion> {
        self.check_sequence(region.commit_seq)?;
        region.coordinate_hash = format!("{:x}", Sha256::digest(region.coordinate_key_material()?));
        let material = self
            .material(&region.namespace_id, &region.material_id)
            .await?;
        let parent = if let Some(id) = &region.parent {
            Some(self.source_region(&region.namespace_id, id).await?)
        } else {
            None
        };
        region.validate_against(&material, parent.as_ref())?;
        if let Some(row) = sqlx::query(
            "SELECT body FROM source_regions WHERE namespace_id=? AND coordinate_hash=?",
        )
        .bind(region.namespace_id.as_str())
        .bind(&region.coordinate_hash)
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(storage_error)?
        {
            return read::decode(row);
        }
        sqlx::query("INSERT INTO source_regions(id,namespace_id,material_id,parent_id,coordinate_hash,body,commit_seq) VALUES(?,?,?,?,?,?,?)")
            .bind(region.id.as_str()).bind(region.namespace_id.as_str()).bind(region.material_id.as_str()).bind(region.parent.as_ref().map(|id|id.as_str()))
            .bind(&region.coordinate_hash).bind(serde_json::to_string(&region)?).bind(region.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(region)
    }
    pub async fn save_representation(
        &mut self,
        representation: &DerivedRepresentation,
        expected_revision: i64,
    ) -> Result<()> {
        self.check_sequence(representation.commit_seq)?;
        self.subject(&representation.namespace_id, &representation.subject_id)
            .await?;
        let current = read::head(
            &mut self.tx,
            &representation.namespace_id,
            &representation.subject_id,
            &representation.id,
        )
        .await?;
        if expected_revision < 0
            || current.as_ref().map_or(0, |row| row.revision) != expected_revision
            || representation.revision
                != expected_revision
                    .checked_add(1)
                    .ok_or_else(|| Error::invalid("representation revision overflow"))?
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "representation head changed",
            ));
        }
        if representation.previous != current.as_ref().map(DerivedRepresentation::reference) {
            return Err(Error::invalid(
                "representation previous does not name its exact head",
            ));
        }
        let mut roots = std::collections::BTreeSet::new();
        for input in &representation.inputs {
            roots.extend(
                graph::roots(
                    &mut self.tx,
                    &representation.namespace_id,
                    &representation.subject_id,
                    &input.source,
                )
                .await?,
            );
        }
        let duration = if let Some(timeline) = &representation.timeline {
            let timeline_roots = graph::roots(
                &mut self.tx,
                &representation.namespace_id,
                &representation.subject_id,
                timeline,
            )
            .await?;
            if !timeline_roots.is_subset(&roots) {
                return Err(Error::invalid(
                    "timeline introduces an unrelated original source",
                ));
            }
            graph::duration(
                &mut self.tx,
                &representation.namespace_id,
                &representation.subject_id,
                timeline,
            )
            .await?
        } else {
            None
        };
        representation.validate(duration)?;
        if let Some(observation) = &representation.observation_id {
            let observation = self
                .observation(&representation.namespace_id, observation)
                .await?;
            if observation.subject_id != representation.subject_id
                || !roots.contains(&observation.material_id)
                || Some(observation.kind) != representation.source_event_kind
            {
                return Err(Error::invalid(
                    "derivation source event differs from its authorized subject and roots",
                ));
            }
        }
        if let Some(id) = &representation.producer.model_call_id {
            let call = self.model_call(id).await?;
            if call.namespace_id != representation.namespace_id
                || call.subject_id != representation.subject_id
                || call.state != ModelCallState::Succeeded
            {
                return Err(Error::invalid(
                    "derivation requires a successful model call in its subject scope",
                ));
            }
        }
        let key = format!(
            "{:x}",
            Sha256::digest(representation.derivation_key_material()?)
        );
        sqlx::query("INSERT INTO media_representations(id,revision,namespace_id,subject_id,derivation_key,body,commit_seq) VALUES(?,?,?,?,?,?,?)")
            .bind(representation.id.as_str()).bind(representation.revision).bind(representation.namespace_id.as_str()).bind(representation.subject_id.as_str())
            .bind(key).bind(serde_json::to_string(representation)?).bind(representation.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
    pub async fn put_derived_region(&mut self, region: DerivedRegion) -> Result<DerivedRegion> {
        self.check_sequence(region.commit_seq)?;
        let representation = self
            .representation(
                &region.namespace_id,
                &region.subject_id,
                &region.representation,
            )
            .await?;
        let parent = if let Some(id) = &region.parent {
            Some(
                self.derived_region(&region.namespace_id, &region.subject_id, id)
                    .await?,
            )
        } else {
            None
        };
        region.validate_against(&representation, parent.as_ref())?;
        let selector = serde_json::to_string(&region.selector)?;
        if let Some(row)=sqlx::query("SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? AND representation_id=? AND representation_revision=? AND coalesce(parent_id,'')=? AND selector=?")
            .bind(region.namespace_id.as_str()).bind(region.subject_id.as_str()).bind(region.representation.id.as_str()).bind(region.representation.revision)
            .bind(region.parent.as_ref().map_or("",|id|id.as_str())).bind(&selector).fetch_optional(&mut *self.tx).await.map_err(storage_error)? {
            return read::decode(row);
        }
        sqlx::query("INSERT INTO derived_regions(id,namespace_id,subject_id,representation_id,representation_revision,parent_id,selector,body,commit_seq) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(region.id.as_str()).bind(region.namespace_id.as_str()).bind(region.subject_id.as_str()).bind(region.representation.id.as_str()).bind(region.representation.revision)
            .bind(region.parent.as_ref().map(|id|id.as_str())).bind(selector).bind(serde_json::to_string(&region)?).bind(region.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(region)
    }
    pub async fn insert_media_exposure(
        &mut self,
        exposure: &MediaExposure,
        actual: ChannelAccess,
    ) -> Result<()> {
        self.check_sequence(exposure.commit_seq)?;
        let observation = self
            .observation(&exposure.namespace_id, &exposure.observation_id)
            .await?;
        let roots = self
            .media_roots(
                &exposure.namespace_id,
                &exposure.subject_id,
                &exposure.locator,
            )
            .await?;
        exposure.validate_against(&observation, &roots, actual)?;
        sqlx::query("INSERT INTO media_exposures(observation_id,namespace_id,subject_id,locator,body,commit_seq) VALUES(?,?,?,?,?,?)")
            .bind(exposure.observation_id.as_str()).bind(exposure.namespace_id.as_str()).bind(exposure.subject_id.as_str())
            .bind(serde_json::to_string(&exposure.locator)?).bind(serde_json::to_string(exposure)?).bind(exposure.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
