use crate::App;
use nous_domain::{media::*, *};
use nous_foundation::models::{MediaData, MessagePart, SourceAccess};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogPayload {
    Text {
        text: String,
    },
    Media {
        blob: BlobReference,
        metadata: MediaMetadata,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaCatalogEntry {
    pub key: String,
    pub source: MediaReference,
    pub role: DerivationInputRole,
    pub channels: ChannelAccess,
    pub at_ms: Option<u64>,
    pub payload: CatalogPayload,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaModelInput {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub source_event_kind: ObservationKind,
    pub strategy: DerivationStrategy,
    pub timeline: Option<MediaReference>,
    pub quality: DerivationQuality,
    pub catalog: Vec<MediaCatalogEntry>,
}
impl MediaModelInput {
    pub fn validate(&self) -> Result<()> {
        if self.catalog.is_empty() || self.catalog.len() > 64 {
            return Err(Error::invalid(
                "media invocation requires 1..64 exact catalog entries",
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut refs = std::collections::BTreeSet::new();
        for entry in &self.catalog {
            entry.source.validate()?;
            if entry.key.is_empty() || !keys.insert(&entry.key) || !refs.insert(&entry.source) {
                return Err(Error::invalid(
                    "media catalog keys and references must be unique",
                ));
            }
            match &entry.payload {
                CatalogPayload::Text { text } if text.trim().is_empty() => {
                    return Err(Error::invalid("media text input is empty"));
                }
                CatalogPayload::Media { blob, metadata } => {
                    blob.validate()?;
                    metadata.validate(&blob.media_type)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn access(&self) -> ChannelAccess {
        merge_access(self.catalog.iter().map(|entry| entry.channels))
    }
    pub fn messages(&self) -> Vec<MessagePart> {
        let mut parts = Vec::new();
        for entry in &self.catalog {
            parts.push(MessagePart::Text {
                text: serde_json::json!({"catalog_key":entry.key,"role":entry.role,"channels":entry.channels,"at_ms":entry.at_ms}).to_string(),
                source: None,
            });
            let source = Some(SourceAccess {
                reference: entry.source.clone(),
                channels: entry.channels,
            });
            parts.push(match &entry.payload {
                CatalogPayload::Text { text } => MessagePart::Text {
                    text: text.clone(),
                    source,
                },
                CatalogPayload::Media { blob, .. } => MessagePart::Media {
                    media_type: blob.media_type.clone(),
                    data: MediaData::Blob { blob: blob.clone() },
                    source,
                },
            });
        }
        parts
    }
}
pub(super) fn merge_access(values: impl Iterator<Item = ChannelAccess>) -> ChannelAccess {
    let mut access = ChannelAccess {
        visual: ChannelMode::Unavailable,
        audio: ChannelMode::Unavailable,
        source_text: ChannelMode::Unavailable,
    };
    let merge = |a, b| match (a, b) {
        (ChannelMode::Original, _) | (_, ChannelMode::Original) => ChannelMode::Original,
        (ChannelMode::Representation, _) | (_, ChannelMode::Representation) => {
            ChannelMode::Representation
        }
        _ => ChannelMode::Unavailable,
    };
    for value in values {
        access = ChannelAccess {
            visual: merge(access.visual, value.visual),
            audio: merge(access.audio, value.audio),
            source_text: merge(access.source_text, value.source_text),
        };
    }
    access
}
impl App {
    pub(super) async fn media_model_input(
        &self,
        run: &MediaDerivationRun,
        include_description: bool,
        cancel: &CancellationToken,
    ) -> Result<MediaModelInput> {
        let (namespace, subject) = run.job.subject_scope()?;
        let ctx = run.job.call_context("nous-media")?;
        let graph = run
            .prepared
            .as_ref()
            .ok_or_else(|| Error::internal("media preparation is missing"))?;
        let mut entries = graph.inputs.clone();
        if include_description {
            let reference = run
                .description
                .clone()
                .ok_or_else(|| Error::internal("committed media description is missing"))?;
            entries.push(MediaPreparedInput {
                source: MediaReference::Representation { reference },
                role: DerivationInputRole::Description,
                at_ms: None,
            });
        }
        let mut catalog = Vec::new();
        for (index, entry) in entries.into_iter().enumerate() {
            let snapshot = self
                .sources
                .resolve(&ctx, namespace, subject, &entry.source)
                .await?;
            let payload = match &snapshot.payload {
                RepresentationPayload::Artifact { blob, metadata }
                    if !matches!(metadata, MediaMetadata::Text) =>
                {
                    if !matches!(snapshot.coordinate, SourceCoordinate::Whole) {
                        return Err(Error::invalid(
                            "model media must reference a committed whole prepared artifact",
                        ));
                    }
                    CatalogPayload::Media {
                        blob: blob.clone(),
                        metadata: metadata.clone(),
                    }
                }
                _ => CatalogPayload::Text {
                    text: self.sources.text(&snapshot, cancel).await?,
                },
            };
            catalog.push(MediaCatalogEntry {
                key: format!("s{index}"),
                source: entry.source,
                role: entry.role,
                channels: snapshot.access,
                at_ms: entry.at_ms,
                payload,
            });
        }
        let input = MediaModelInput {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            source_event_kind: run.source_event_kind,
            strategy: run.request.strategy,
            timeline: Some(run.request.source.clone()),
            quality: graph.quality.clone(),
            catalog,
        };
        input.validate()?;
        Ok(input)
    }
}
