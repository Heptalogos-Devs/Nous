//! Text selection is shared by source reads and embedding authority validation.
use super::*;

pub fn representation_text(
    payload: &RepresentationPayload,
    selector: Option<&DerivedSelector>,
) -> Result<Option<String>> {
    match (payload, selector) {
        (RepresentationPayload::Artifact { .. }, None | Some(DerivedSelector::Artifact { .. })) => {
            Ok(None)
        }
        (RepresentationPayload::Text { text }, None) => Ok(Some(text.clone())),
        (RepresentationPayload::Text { text }, Some(DerivedSelector::TextSpan { start, end })) => {
            let start = usize::try_from(*start)
                .map_err(|_| Error::invalid("text offset exceeds platform range"))?;
            let end = usize::try_from(*end)
                .map_err(|_| Error::invalid("text offset exceeds platform range"))?;
            Ok(Some(
                text.get(start..end)
                    .ok_or_else(|| {
                        Error::invalid("derived text span is outside UTF-8 text boundaries")
                    })?
                    .to_owned(),
            ))
        }
        (RepresentationPayload::Interpretation { value }, None) => {
            Ok(Some(serde_json::to_string(value)?))
        }
        (
            RepresentationPayload::Interpretation { value },
            Some(DerivedSelector::Interpretation { item }),
        ) => {
            let selected = match item {
                InterpretationSelector::Summary => serde_json::to_value(&value.summary)?,
                InterpretationSelector::Observation { index } => {
                    item_value(&value.observations, *index)?
                }
                InterpretationSelector::Mention { index } => item_value(&value.mentions, *index)?,
                InterpretationSelector::EmbeddedText { index } => {
                    item_value(&value.embedded_text, *index)?
                }
                InterpretationSelector::SourceText { index } => {
                    item_value(&value.source_text, *index)?
                }
                InterpretationSelector::Speech { index } => item_value(&value.speech, *index)?,
                InterpretationSelector::Interpretation { index } => {
                    item_value(&value.interpretations, *index)?
                }
                InterpretationSelector::Uncertainty { index } => {
                    item_value(&value.uncertainties, *index)?
                }
            };
            Ok(Some(serde_json::to_string(&selected)?))
        }
        _ => Err(Error::invalid(
            "derived selector does not select this payload kind",
        )),
    }
}
fn item_value<T: Serialize>(items: &[T], index: u32) -> Result<serde_json::Value> {
    serde_json::to_value(items.get(index as usize).ok_or_else(|| {
        Error::new(
            crate::ErrorCode::NotFound,
            "interpretation item does not exist",
        )
    })?)
    .map_err(Error::from)
}
