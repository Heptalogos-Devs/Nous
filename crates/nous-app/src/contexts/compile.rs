use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) fn hash(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub(super) fn segments(
    preparation: &mut ContextPreparation,
    proposal: Option<&model::StewardProposal>,
    call: Option<&ModelCallId>,
) -> Result<Vec<ContextSegment>> {
    let mut selected = preparation
        .catalog
        .iter()
        .filter(|source| source.required)
        .cloned()
        .collect::<Vec<_>>();
    let optional = preparation
        .catalog
        .iter()
        .filter(|source| !source.required)
        .collect::<Vec<_>>();
    if let Some(proposal) = proposal {
        let sources = optional
            .into_iter()
            .filter(|source| proposal.selected_ids.contains(&source.key))
            .collect::<Vec<_>>();
        if let Some(summary) = proposal
            .summary
            .as_ref()
            .filter(|text| !text.trim().is_empty())
        {
            selected.push(ContextSegment {
                key: "steward".into(),
                text: summary.clone(),
                sources: sources
                    .iter()
                    .flat_map(|source| source.sources.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                dependencies: sources
                    .iter()
                    .flat_map(|source| source.dependencies.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                required: false,
                authority: ContextAuthority::StewardInterpretation,
                provenance: json!({"model_call_id":call,"selected_ids":proposal.selected_ids}),
                epistemic: Some(ContextEpistemic {
                    stance: Some(EpistemicStance {
                        class: EpistemicClass::Inferred,
                        attribution: Some("steward".into()),
                        confidence: None,
                    }),
                    cognitive_role: None,
                    grounding_kind: None,
                    source_event: None,
                    qualifications: Vec::new(),
                    uncertain: false,
                }),
            });
        } else {
            selected.extend(sources.into_iter().cloned());
        }
    } else {
        selected.extend(optional.into_iter().cloned());
    }
    let policy = preparation
        .policy
        .as_ref()
        .ok_or_else(|| Error::internal("context policy snapshot is missing"))?;
    let max_items = policy.max_items.min(preparation.request.max_items);
    let max_bytes = policy
        .max_text_bytes
        .min(preparation.request.max_text_bytes);
    let mut bytes = 0;
    let mut output = Vec::new();
    for segment in selected {
        if output.len() < max_items && bytes + segment.text.len() <= max_bytes {
            bytes += segment.text.len();
            output.push(segment);
        } else if segment.required {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "required context unit does not fit",
            ));
        } else {
            preparation
                .degradation
                .push("optional semantic unit omitted whole by contribution budget".into());
        }
    }
    Ok(output)
}
pub(super) fn epistemic_view(
    segments: &[ContextSegment],
    coverage: &[String],
) -> ContextEpistemicView {
    let mut view = ContextEpistemicView {
        coverage: coverage.to_vec(),
        ..Default::default()
    };
    for segment in segments {
        if matches!(
            segment.authority,
            ContextAuthority::SourceMaterial | ContextAuthority::StewardInterpretation
        ) {
            view.execution_assistance.push(segment.key.clone());
        }
        if segment.authority == ContextAuthority::RecordedCognition {
            view.accepted_facts_and_reports.push(segment.key.clone());
        }
        if let Some(epistemic) = &segment.epistemic {
            if epistemic.uncertain {
                view.unknowns.push(segment.key.clone());
            }
            if matches!(
                epistemic.cognitive_role,
                Some(CognitiveRole::Experiential | CognitiveRole::ProceduralExperience)
            ) && epistemic.grounding_kind == Some(ObservationKind::Contact)
            {
                view.experiences.push(segment.key.clone());
            }
        }
    }
    view
}
pub(super) fn response(
    preparation: &ContextPreparation,
    projection: ContextProjection,
    previous: Option<ContextProjection>,
) -> Result<ManagedContextResponse> {
    let mut projection = projection;
    let mut usage = preparation.request.usage.clone();
    let time_domain = usage.world_time.as_ref().map(|point| point.domain.clone());
    usage.world_time = None;
    let track = hash(
        &json!({"namespace":projection.namespace_id,"subject":projection.subject_id,"session":projection.session_id,"consumer":projection.consumer_id,"instance":projection.instance_id,"usage":usage,"time_domain":time_domain}),
    )?;
    let known = &preparation.request.known_cursor;
    let appendable = previous.as_ref().is_some_and(|previous| {
        known.as_ref() == Some(&previous.cursor)
            && previous.cursor.track_id == track
            && previous.config_digest == projection.config_digest
            && previous.segments.len() <= projection.segments.len()
            && previous
                .segments
                .iter()
                .zip(&projection.segments)
                .all(|(left, right)| {
                    left.text == right.text
                        && left.sources == right.sources
                        && left.dependencies == right.dependencies
                        && left.authority == right.authority
                        && left.required == right.required
                })
    });
    let (kind, epoch, revision, prefix) = if appendable {
        let previous = previous.as_ref().expect("appendable previous");
        let delta = projection.segments.len() > previous.segments.len();
        (
            ContextPatchKind::Append,
            previous.cursor.epoch_id.clone(),
            previous
                .cursor
                .revision
                .checked_add(u64::from(delta))
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceExhausted, "context cursor exhausted")
                })?,
            previous.segments.len(),
        )
    } else {
        (
            ContextPatchKind::Reset,
            uuid::Uuid::new_v4().to_string(),
            1,
            0,
        )
    };
    projection.cursor = ContextCursor {
        track_id: track,
        epoch_id: epoch,
        revision,
        projection_id: projection.id.clone(),
    };
    let mut delivered = projection.clone();
    delivered.segments = delivered.segments.into_iter().skip(prefix).collect();
    Ok(ManagedContextResponse {
        kind,
        cursor: projection.cursor,
        projection: delivered,
    })
}
