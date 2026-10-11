// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;

pub struct DescriptionSegment {
    pub key: String,
    pub text: String,
    pub region: DerivedRegion,
}

const ABSOLUTE_DESCRIPTION_BYTES: usize = 1_048_576;
const ABSOLUTE_DESCRIPTION_REGIONS: usize = 512;

fn ranges(text: &str, segment_bytes: usize) -> Result<Vec<(usize, usize)>> {
    if text.is_empty() || text.len() > ABSOLUTE_DESCRIPTION_BYTES {
        return Err(Error::new(
            nous_core::ErrorOwner::Material,
            nous_core::ErrorCode::InvalidRequest,
            "description segmentation requires bounded text",
        ));
    }
    let mut result = vec![];
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + segment_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if let Some(newline) = text[start..end].find('\n') {
            end = start + newline + 1;
        }
        result.push((start, end));
        if result.len() > ABSOLUTE_DESCRIPTION_REGIONS {
            return Err(Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "description segmentation exceeds 512 regions",
            ));
        }
        start = end;
    }
    Ok(result)
}

impl MaterialService {
    pub async fn segment_description(
        &self,
        subject: SubjectId,
        representation: DerivedRepresentationId,
    ) -> Result<Vec<DescriptionSegment>> {
        let snapshot = self.cognition.configuration.snapshot_for_subject(subject)?;
        let segment_bytes = snapshot.get(crate::DESCRIPTION_SEGMENT_BYTES)?;
        let config_digest = snapshot.digest_for(&[crate::DESCRIPTION_SEGMENT_BYTES.path()])?;
        let mut tx = self.store.begin().await?;
        let text: Option<String> = sqlx::query_scalar("SELECT payload_text FROM derived_representations WHERE subject_id=$1 AND derived_representation_id=$2 FOR SHARE")
            .bind(subject.0).bind(representation.0).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(|| Error::new(nous_core::ErrorOwner::Material, nous_core::ErrorCode::NotFound, "description representation not found"))?;
        let text = text.ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Material,
                nous_core::ErrorCode::InvalidRequest,
                "description has no text projection",
            )
        })?;
        let mut segments = vec![];
        for (ordinal, (start, end)) in ranges(&text, segment_bytes)?.into_iter().enumerate() {
            let slice = &text[start..end];
            let coordinate = serde_json::json!({"algorithm":"description-utf8-lines-v1","config_digest":config_digest,"ordinal":ordinal,"utf8_start":start,"utf8_end":end,"digest":blake3::hash(slice.as_bytes()).to_hex().to_string()});
            let mut region = DerivedRegion {
                derived_region_id: DerivedRegionId::new(),
                subject_id: subject,
                derived_representation_id: representation,
                coordinate_kind: "description_segment".into(),
                coordinate_hash: blake3::hash(coordinate.to_string().as_bytes())
                    .to_hex()
                    .to_string(),
                coordinate,
                parent_derived_region_id: None,
                created_at: self.cognition.now(subject),
            };
            region.derived_region_id = self.insert_derived_region_in_tx(&mut tx, &region).await?;
            segments.push(DescriptionSegment {
                key: format!("D{:03}", ordinal + 1),
                text: slice.into(),
                region,
            });
        }
        tx.commit().await.map_err(db)?;
        Ok(segments)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/service/segmentation.rs"]
mod tests;
