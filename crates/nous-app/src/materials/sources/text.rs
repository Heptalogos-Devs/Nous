use super::*;

pub(super) fn select(source: &mut SourceSnapshot, selector: DerivedSelector) -> Result<()> {
    match (&source.payload, selector) {
        (RepresentationPayload::Text { text }, DerivedSelector::TextSpan { start, end }) => {
            let selected = text
                .get(
                    usize::try_from(start)
                        .map_err(|_| Error::invalid("text offset exceeds platform range"))?
                        ..usize::try_from(end)
                            .map_err(|_| Error::invalid("text offset exceeds platform range"))?,
                )
                .ok_or_else(|| {
                    Error::invalid("derived text span is outside UTF-8 text boundaries")
                })?
                .to_owned();
            source.payload = RepresentationPayload::Text { text: selected };
        }
        (
            RepresentationPayload::Interpretation { value },
            DerivedSelector::Interpretation { item },
        ) => {
            let selected = match item {
                InterpretationSelector::Summary => serde_json::to_value(&value.summary)?,
                InterpretationSelector::Observation { index } => {
                    item_value(&value.observations, index)?
                }
                InterpretationSelector::Mention { index } => item_value(&value.mentions, index)?,
                InterpretationSelector::EmbeddedText { index } => {
                    item_value(&value.embedded_text, index)?
                }
                InterpretationSelector::SourceText { index } => {
                    item_value(&value.source_text, index)?
                }
                InterpretationSelector::Speech { index } => item_value(&value.speech, index)?,
                InterpretationSelector::Interpretation { index } => {
                    item_value(&value.interpretations, index)?
                }
                InterpretationSelector::Uncertainty { index } => {
                    item_value(&value.uncertainties, index)?
                }
            };
            source.payload = RepresentationPayload::Text {
                text: serde_json::to_string(&selected)?,
            };
        }
        (RepresentationPayload::Artifact { .. }, DerivedSelector::Artifact { coordinate }) => {
            if let SourceCoordinate::TimeSpan { start_ms, end_ms } = &coordinate {
                source.timeline = Some(source.reference.clone());
                source.quality.source_duration_ms = Some(end_ms - start_ms);
                source.quality.sampling = None;
            }
            source.quality.partial |= !matches!(coordinate, SourceCoordinate::Whole);
            source.coordinate = coordinate;
        }
        _ => {
            return Err(Error::invalid(
                "derived selector does not select this payload kind",
            ));
        }
    }
    Ok(())
}
fn item_value<T: Serialize>(items: &[T], index: u32) -> Result<serde_json::Value> {
    serde_json::to_value(
        items
            .get(index as usize)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "interpretation item does not exist"))?,
    )
    .map_err(Error::from)
}
