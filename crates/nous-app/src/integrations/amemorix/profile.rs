use super::types::*;
use crate::App;
use nous_domain::*;
use serde_json::{Value, json};

pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ProfileInput,
) -> Result<Value> {
    if input.limit == 0 {
        return Err(Error::invalid("profile evidence limit must be positive"));
    }
    let mut read = app.database.read_view().await?;
    let source_read = read
        .source_read_view(ctx, &binding.namespace_id, &binding.subject_id)
        .await?;
    let mut matches = Vec::new();
    let references = if !input.person_id.is_empty() {
        read.resolve_entity_identifier(
            &binding.namespace_id,
            &binding.subject_id,
            "maibot:person",
            &input.person_id,
            None,
        )
        .await?
    } else if !input.person_name.is_empty() {
        vec![
            read.resolve_query_identity(
                &binding.namespace_id,
                &binding.subject_id,
                CognitiveKind::Entity,
                &input.person_name,
                &nous_storage::ReferenceViewOptions {
                    as_of: None,
                    source_read: Some(&source_read),
                },
            )
            .await?
            .reference,
        ]
    } else {
        return Err(Error::invalid("profile requires person_id or person_name"));
    };
    for reference in references {
        if read
            .source_read_allowed(&source_read, &RetrievalTarget::Cognition(reference.clone()))
            .await?
        {
            matches.push(reference);
        }
    }
    if matches.is_empty() {
        return Err(Error::new(
            ErrorCode::NotFound,
            "person has no authorized Entity profile",
        ));
    }
    if matches.len() > 1 {
        return Err(Error::new(
            ErrorCode::Conflict,
            "person identity has multiple Entity profiles",
        )
        .with_context("candidates", json!(matches)));
    }
    let reference = matches.remove(0);
    if !input.person_id.is_empty() && !input.person_name.is_empty() {
        let named = read
            .resolve_query_identity(
                &binding.namespace_id,
                &binding.subject_id,
                CognitiveKind::Entity,
                &input.person_name,
                &nous_storage::ReferenceViewOptions {
                    as_of: None,
                    source_read: Some(&source_read),
                },
            )
            .await?;
        if named.reference.id != reference.id {
            return Err(Error::new(
                ErrorCode::Conflict,
                "person name and identifier do not select the same Entity",
            ));
        }
    }
    let object = read
        .structured_revision(
            &binding.namespace_id,
            &CognitiveId(reference.id.clone()),
            reference.revision,
        )
        .await?;
    read.finish().await?;
    let source = app
        .retrieval_source(
            ctx,
            &binding.subject_id,
            &RetrievalTarget::Cognition(reference.clone()),
        )
        .await?;
    if source.pointer("/closure/complete").and_then(Value::as_bool) == Some(false) {
        return Err(Error::new(
            ErrorCode::NotFound,
            "person profile evidence contains a removed source",
        )
        .with_context("reference", reference.to_string())
        .with_context(
            "source_closure",
            source.get("closure").cloned().unwrap_or(Value::Null),
        ));
    }
    let CognitiveContent::Entity(entity) = object.body else {
        return Err(Error::internal(
            "person identifier resolved to a non-Entity record",
        ));
    };
    if entity.entity_type != EntityType::Person {
        return Err(Error::new(
            ErrorCode::Conflict,
            "person identifier is bound to a non-person Entity",
        ));
    }
    let now = nous_foundation::clock::Clock::operation_snapshot().point();
    let mut attributes = entity.attributes.iter().enumerate().collect::<Vec<_>>();
    attributes.sort_by(|(left_index, left), (right_index, right)| {
        let left = nous_retrieval::match_score(
            &input.context_text,
            &format!("{} {}", left.predicate, left.value),
        )
        .unwrap_or(0.0);
        let right = nous_retrieval::match_score(
            &input.context_text,
            &format!("{} {}", right.predicate, right.value),
        )
        .unwrap_or(0.0);
        right.total_cmp(&left).then(left_index.cmp(right_index))
    });
    let mut traits = Vec::new();
    let mut evidence = Vec::new();
    let mut uncertain = Vec::new();
    for (_, attribute) in attributes {
        let text = format!("{}: {}", attribute.predicate, attribute.value);
        let eligible = matches!(
            attribute.epistemic.class,
            EpistemicClass::Observed | EpistemicClass::Reported
        ) && !attribute
            .evidence
            .iter()
            .any(|link| link.polarity == EvidencePolarity::Contradict)
            && attribute
                .validity
                .as_ref()
                .is_none_or(|range| range.domain == now.domain && range.contains(&now));
        if !eligible {
            if uncertain.len() < input.limit {
                uncertain.push(json!({"text":text,"epistemic":attribute.epistemic,"validity":attribute.validity,"evidence":attribute.evidence}));
            }
            continue;
        }
        if traits.len() < input.limit {
            traits.push(text.clone());
            evidence.push(json!({"hash":reference.to_string(),"content":text,"type":"entity_attribute","metadata":{"epistemic":attribute.epistemic,"validity":attribute.validity},"evidence":attribute.evidence}));
        }
    }
    let summary = std::iter::once(entity.description.clone())
        .chain(traits.iter().cloned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(
        json!({"success":true,"person_id":input.person_id,"person_name":entity.label,"summary":summary,"profile_text":summary,"traits":traits,"evidence":evidence,"uncertain_candidates":uncertain,"reference":reference,"profile_source":"entity","context":{"chat_id":input.chat_id,"text":input.context_text},"source_closure":source.get("closure"),"coverage":{"total_attributes":entity.attributes.len(),"selected_attributes":traits.len(),"limit":input.limit}}),
    )
}
