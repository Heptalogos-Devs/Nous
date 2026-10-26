use super::input::{MediaModelInput, merge_access};
use nous_domain::{media::*, *};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawInterpretation {
    pub summary: RawSummary,
    pub coverage: InterpretationCoverage,
    pub observations: Vec<RawObservation>,
    pub mentions: Vec<RawMention>,
    pub embedded_text: Vec<RawTextSpan>,
    pub source_text: Vec<RawTextSpan>,
    pub speech: Vec<RawSpeechSpan>,
    pub interpretations: Vec<RawClaim>,
    pub uncertainties: Vec<RawUncertainty>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSummary {
    pub content: String,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawObservation {
    pub kind: MediaObservationKind,
    pub content: String,
    pub evidence_channel: EvidenceChannel,
    pub basis: InterpretationBasis,
    pub certainty: InterpretationCertainty,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawMention {
    pub surface: String,
    pub category: MentionCategory,
    pub role: Option<String>,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawTextSpan {
    pub text: String,
    pub fidelity: TextFidelity,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawSpeechSpan {
    pub text: String,
    pub fidelity: SpeechFidelity,
    pub speaker_hint: Option<String>,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawClaim {
    pub content: String,
    pub status: InterpretationStatus,
    pub basis_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RawUncertainty {
    pub issue: String,
    pub alternatives: Vec<String>,
    pub basis_refs: Vec<String>,
}

fn references(keys: &[String], input: &MediaModelInput) -> Result<Vec<MediaReference>> {
    keys.iter()
        .map(|key| {
            input
                .catalog
                .iter()
                .find(|entry| entry.key == *key)
                .map(|entry| entry.source.clone())
                .ok_or_else(|| Error::invalid(format!("unknown media catalog key: {key}")))
        })
        .collect()
}
fn check_channel(
    keys: &[String],
    channel: EvidenceChannel,
    direct: bool,
    input: &MediaModelInput,
) -> Result<()> {
    let mut entries = Vec::new();
    for key in keys {
        entries.push(
            input
                .catalog
                .iter()
                .find(|entry| entry.key == *key)
                .ok_or_else(|| Error::invalid(format!("unknown media catalog key: {key}")))?,
        );
    }
    let access = merge_access(entries.into_iter().map(|entry| entry.channels));
    if !keys.is_empty()
        && (access.mode(channel) == ChannelMode::Unavailable
            || (direct && access.mode(channel) != ChannelMode::Original))
    {
        return Err(Error::invalid(
            "interpretation basis does not supply the claimed actual channel",
        ));
    }
    Ok(())
}
impl RawInterpretation {
    pub fn compile(self, input: &MediaModelInput) -> Result<MediaInterpretation> {
        let summary = MediaSummary {
            content: self.summary.content,
            basis_refs: references(&self.summary.basis_refs, input)?,
        };
        let observations = self
            .observations
            .into_iter()
            .map(|item| {
                check_channel(
                    &item.basis_refs,
                    item.evidence_channel,
                    item.basis == InterpretationBasis::Direct,
                    input,
                )?;
                Ok(MediaObservation {
                    kind: item.kind,
                    content: item.content,
                    evidence_channel: item.evidence_channel,
                    basis: item.basis,
                    certainty: item.certainty,
                    start_ms: item.start_ms,
                    end_ms: item.end_ms,
                    basis_refs: references(&item.basis_refs, input)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mentions = self
            .mentions
            .into_iter()
            .map(|item| {
                Ok(MediaMention {
                    surface: item.surface,
                    category: item.category,
                    role: item.role,
                    basis_refs: references(&item.basis_refs, input)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let text = |items: Vec<RawTextSpan>, channel| {
            items
                .into_iter()
                .map(|item| {
                    check_channel(&item.basis_refs, channel, false, input)?;
                    Ok(MediaTextSpan {
                        text: item.text,
                        fidelity: item.fidelity,
                        start_ms: item.start_ms,
                        end_ms: item.end_ms,
                        basis_refs: references(&item.basis_refs, input)?,
                    })
                })
                .collect::<Result<Vec<_>>>()
        };
        let embedded_text = text(self.embedded_text, EvidenceChannel::Visual)?;
        let source_text = text(self.source_text, EvidenceChannel::SourceText)?;
        let speech = self
            .speech
            .into_iter()
            .map(|item| {
                check_channel(&item.basis_refs, EvidenceChannel::Audio, false, input)?;
                Ok(MediaSpeechSpan {
                    text: item.text,
                    fidelity: item.fidelity,
                    speaker_hint: item.speaker_hint,
                    start_ms: item.start_ms,
                    end_ms: item.end_ms,
                    basis_refs: references(&item.basis_refs, input)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let interpretations = self
            .interpretations
            .into_iter()
            .map(|item| {
                Ok(MediaClaim {
                    content: item.content,
                    status: item.status,
                    basis_refs: references(&item.basis_refs, input)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let uncertainties = self
            .uncertainties
            .into_iter()
            .map(|item| {
                Ok(MediaUncertainty {
                    issue: item.issue,
                    alternatives: item.alternatives,
                    basis_refs: references(&item.basis_refs, input)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let output = MediaInterpretation {
            summary,
            coverage: self.coverage,
            observations,
            mentions,
            embedded_text,
            source_text,
            speech,
            interpretations,
            uncertainties,
        };
        let catalog = input
            .catalog
            .iter()
            .map(|entry| entry.source.clone())
            .collect::<BTreeSet<_>>();
        output.validate(input.access(), &catalog, input.quality.source_duration_ms)?;
        Ok(output)
    }
}

pub(super) fn schema(input: &MediaModelInput) -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(RawInterpretation))
        .expect("media schema serializes");
    let keys = input
        .catalog
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    constrain(&mut schema, &keys, input.quality.source_duration_ms);
    schema
}
fn constrain(schema: &mut Value, keys: &[String], duration: Option<u64>) {
    match schema {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("array") {
                map.insert("maxItems".into(), json!(64));
            }
            if let Some(Value::Object(properties)) = map.get_mut("properties") {
                if let Some(basis) = properties.get_mut("basis_refs") {
                    basis["items"] = json!({"type":"string","enum":keys});
                }
                for key in ["start_ms", "end_ms"] {
                    if let Some(time) = properties.get_mut(key) {
                        *time = duration.map_or_else(|| json!({"type":"null"}), |max| json!({"anyOf":[{"type":"integer","minimum":0,"maximum":max},{"type":"null"}]}));
                    }
                }
            }
            for value in map.values_mut() {
                constrain(value, keys, duration);
            }
        }
        Value::Array(values) => {
            for value in values {
                constrain(value, keys, duration);
            }
        }
        _ => {}
    }
}
