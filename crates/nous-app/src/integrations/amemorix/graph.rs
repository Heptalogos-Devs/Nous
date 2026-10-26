use super::types::*;
use crate::App;
use nous_domain::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn adopt(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &IngestInput,
    memory: &Memory,
) -> Result<Vec<CognitiveRef>> {
    let mut output = Vec::new();
    let mut names = input
        .entities
        .iter()
        .chain(&input.participants)
        .filter(|name| !name.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relation in &input.relations {
        names.insert(relation.subject.clone());
        names.insert(relation.object.clone());
    }
    let mut entities = BTreeMap::new();
    for name in names {
        let content = EntityContent {
            entity_type: EntityType::Other,
            label: name.clone(),
            aliases: vec![],
            description: String::new(),
            identifiers: vec![ExternalEntityIdentity {
                system: "amemorix:entity".into(),
                key: name.clone(),
            }],
            mentions: vec![EntityMention {
                target: SemanticTarget::from_reference(RetrievalTarget::Cognition(
                    CognitiveRef::memory(&memory.id, 1),
                )),
                role: "source_mention".into(),
            }],
            attributes: vec![],
        };
        let reference = entity(app, ctx, binding, input, memory, content).await?;
        entities.insert(name, reference.clone());
        output.push(reference);
    }
    for person in &input.person_ids {
        let attributes = if input.source_type == "person_fact" {
            vec![EntityAttribute {
                predicate: "reported_fact".into(),
                value: input.text.clone(),
                epistemic: EpistemicStance {
                    class: EpistemicClass::Reported,
                    attribution: Some("amemorix:person_fact".into()),
                    confidence: None,
                },
                validity: None,
                evidence: vec![evidence(memory)],
            }]
        } else {
            vec![]
        };
        let content = EntityContent {
            entity_type: EntityType::Person,
            label: input
                .metadata
                .get("person_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(person)
                .to_owned(),
            aliases: vec![],
            description: String::new(),
            identifiers: vec![ExternalEntityIdentity {
                system: "maibot:person".into(),
                key: person.clone(),
            }],
            mentions: vec![EntityMention {
                target: SemanticTarget::from_reference(RetrievalTarget::Cognition(
                    CognitiveRef::memory(&memory.id, 1),
                )),
                role: "person_source".into(),
            }],
            attributes,
        };
        output.push(entity(app, ctx, binding, input, memory, content).await?);
    }
    for (index, relation) in input.relations.iter().enumerate() {
        if relation.subject.is_empty()
            || relation.predicate.is_empty()
            || relation.object.is_empty()
        {
            return Err(Error::invalid(
                "relation needs subject, predicate and object",
            ));
        }
        if relation
            .confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(Error::invalid("relation confidence must be in 0..1"));
        }
        let body = CognitiveContent::Association(AssociationContent {
            from: entities[&relation.subject].clone(),
            to: entities[&relation.object].clone(),
            relation: relation.predicate.clone(),
            polarity: AssociationPolarity::Positive,
            basis_class: AssociationBasisClass::HostExplicit,
            description: relation
                .metadata
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
        let receipt = app
            .create_structured(
                ctx,
                &step(&input.external_id, &format!("relation:{index}")),
                CreateStructuredRequest {
                    namespace_id: binding.namespace_id.clone(),
                    subject_id: binding.subject_id.clone(),
                    expected_revision: 0,
                    body,
                    evidence: vec![evidence(memory)],
                    times: memory.times.clone(),
                    producer_call_id: None,
                },
            )
            .await?;
        let object: StructuredObject = serde_json::from_value(receipt.result)?;
        output.push(object.reference());
    }
    Ok(output)
}
async fn entity(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &IngestInput,
    memory: &Memory,
    mut content: EntityContent,
) -> Result<CognitiveRef> {
    let identity = &content.identifiers[0];
    let idempotency = step(
        &input.external_id,
        &format!("entity:{}:{}", identity.system, identity.key),
    );
    let mut read = app.database.read_view().await?;
    let matches = read
        .resolve_entity_identifier(
            &binding.namespace_id,
            &binding.subject_id,
            &identity.system,
            &identity.key,
            Some(memory.commit_seq),
        )
        .await?;
    if matches.len() > 1 {
        return Err(
            Error::new(ErrorCode::Conflict, "external entity identity is ambiguous")
                .with_context("candidates", json!(matches)),
        );
    }
    let previous = if let Some(reference) = matches.first() {
        Some(
            read.structured_revision(
                &binding.namespace_id,
                &CognitiveId(reference.id.clone()),
                reference.revision,
            )
            .await?,
        )
    } else {
        None
    };
    read.finish().await?;
    let receipt = if let Some(previous) = previous {
        let CognitiveContent::Entity(mut current) = previous.body else {
            return Err(Error::internal(
                "entity resolver returned a different cognitive kind",
            ));
        };
        for mention in content.mentions.drain(..) {
            if !current
                .mentions
                .iter()
                .any(|item| item.target == mention.target && item.role == mention.role)
            {
                current.mentions.push(mention);
            }
        }
        current.attributes.append(&mut content.attributes);
        let mut sources = previous.evidence;
        sources.push(evidence(memory));
        app.revise_structured(
            ctx,
            &idempotency,
            ReviseStructuredRequest {
                id: previous.id,
                expected_revision: previous.revision,
                body: CognitiveContent::Entity(current),
                evidence: sources,
                times: previous.times,
                producer_call_id: None,
            },
        )
        .await?
    } else {
        app.create_structured(
            ctx,
            &idempotency,
            CreateStructuredRequest {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
                expected_revision: 0,
                body: CognitiveContent::Entity(content),
                evidence: vec![evidence(memory)],
                times: memory.times.clone(),
                producer_call_id: None,
            },
        )
        .await?
    };
    let object: StructuredObject = serde_json::from_value(receipt.result)?;
    Ok(object.reference())
}
fn evidence(memory: &Memory) -> EvidenceLink {
    EvidenceLink {
        source: EvidenceSource::MemoryRevision {
            memory_id: memory.id.clone(),
            revision: 1,
        },
        polarity: EvidencePolarity::Support,
        relation: None,
        explanation: String::new(),
    }
}
pub(super) fn step(external: &str, suffix: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("amemorix:{external}:{suffix}"))
    )
}
