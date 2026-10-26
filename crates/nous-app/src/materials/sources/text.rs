use super::*;

pub(super) fn select(source: &mut SourceSnapshot, selector: DerivedSelector) -> Result<()> {
    if let DerivedSelector::Artifact { coordinate } = selector {
        if !matches!(source.payload, RepresentationPayload::Artifact { .. }) {
            return Err(Error::invalid(
                "derived selector does not select this payload kind",
            ));
        }
        if let SourceCoordinate::TimeSpan { start_ms, end_ms } = &coordinate {
            source.timeline = Some(source.reference.clone());
            source.quality.source_duration_ms = Some(end_ms - start_ms);
            source.quality.sampling = None;
        }
        source.quality.partial |= !matches!(coordinate, SourceCoordinate::Whole);
        source.coordinate = coordinate;
    } else {
        let text = representation_text(&source.payload, Some(&selector))?
            .ok_or_else(|| Error::invalid("derived selector does not produce text"))?;
        source.payload = RepresentationPayload::Text { text };
    }
    Ok(())
}
