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
        return Err(Error::Invalid(
            "description segmentation requires bounded text".into(),
        ));
    }
    let mut result = vec![];
    let mut start = 0;
    while start < text.len() {
        let mut end = text.floor_char_boundary(start + segment_bytes);
        if let Some(newline) = text[start..end].find('\n') {
            end = start + newline + 1;
        }
        result.push((start, end));
        if result.len() > ABSOLUTE_DESCRIPTION_REGIONS {
            return Err(Error::Invalid(
                "description segmentation exceeds 512 regions".into(),
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
            .ok_or_else(|| Error::NotFound("description representation not found".into()))?;
        let text =
            text.ok_or_else(|| Error::Invalid("description has no text projection".into()))?;
        let mut segments = vec![];
        for (ordinal, (start, end)) in ranges(&text, segment_bytes)?.into_iter().enumerate() {
            let slice = &text[start..end];
            let coordinate = serde_json::json!({"algorithm":"description-utf8-lines-v1","config_digest":config_digest,"ordinal":ordinal,"utf8_start":start,"utf8_end":end,"digest":blake3::hash(slice.as_bytes()).to_hex().to_string()});
            let region = DerivedRegion {
                formed_stamp: None,
                derived_region_id: DerivedRegionId::new(),
                subject_id: subject,
                derived_representation_id: representation,
                coordinate_kind: "description_segment".into(),
                coordinate_hash: blake3::hash(coordinate.to_string().as_bytes())
                    .to_hex()
                    .to_string(),
                coordinate,
                parent_derived_region_id: None,
                created_at: self.cognition.recorded_now(),
            };
            segments.push(DescriptionSegment {
                key: format!("D{:03}", ordinal + 1),
                text: slice.into(),
                region,
            });
        }
        let regions = self
            .insert_derived_regions_in_tx(
                &mut tx,
                &segments
                    .iter()
                    .map(|segment| segment.region.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
        for (segment, region) in segments.iter_mut().zip(regions) {
            segment.region = region;
        }
        tx.commit().await.map_err(db)?;
        Ok(segments)
    }
}

#[cfg(test)]
mod tests {
    use super::ranges;
    #[test]
    fn segmentation_preserves_utf8_bytes_and_line_order() {
        let text = format!("first\n\n{}\nlast", "语音🙂".repeat(600));
        let spans = ranges(&text, 2048).unwrap();
        assert_eq!(
            spans.iter().map(|(a, b)| &text[*a..*b]).collect::<String>(),
            text
        );
        assert!(spans.iter().all(|(a, b)| b - a <= 2048));
        assert_eq!(&text[spans[0].0..spans[0].1], "first\n");
        assert_eq!(spans, ranges(&text, 2048).unwrap());
    }
}
