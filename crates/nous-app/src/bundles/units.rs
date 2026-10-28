use crate::App;
use nous_domain::{media::*, *};
use tokio_util::sync::CancellationToken;

impl App {
    /// Both export and installation use the existing strict media range reader.
    /// Installation invokes this after archive objects have been verified and published.
    pub(super) async fn verify_bundle_unit_sources(
        &self,
        package: &SubjectPackage,
        cancel: &CancellationToken,
    ) -> Result<()> {
        for unit in &package.text_units {
            let source = package
                .input_sources
                .iter()
                .find(|value| value.id == unit.input_source_id)
                .ok_or_else(|| Error::invalid("text unit has no declared input source"))?;
            let material = package
                .materials
                .iter()
                .find(|value| value.material.id == source.material_id)
                .ok_or_else(|| Error::invalid("unit original material is absent"))?;
            let blob = BlobReference {
                content_hash: source.content_hash.clone(),
                media_type: material.material.media_type.clone(),
                byte_len: source.byte_len,
            };
            for reference in unit.source_requirements() {
                let SourceReadRef::Media {
                    reference: MediaReference::SourceRegion { id },
                    ..
                } = reference
                else {
                    unreachable!()
                };
                let region = package
                    .source_regions
                    .iter()
                    .find(|value| value.id == id)
                    .ok_or_else(|| Error::invalid("unit text or context region is absent"))?;
                let SourceCoordinate::TextSpan { start, end } = region.coordinate else {
                    return Err(Error::invalid("unit source requires a UTF-8 TextSpan"));
                };
                let actual = self
                    .media
                    .read_text_range(&blob, start..end, cancel)
                    .await?;
                if id == unit.source_region_id && actual != unit.text {
                    return Err(Error::invalid(
                        "unit text differs from its actual UTF-8 original bytes",
                    ));
                }
            }
        }
        Ok(())
    }
}
