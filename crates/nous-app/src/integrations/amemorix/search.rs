use super::{policy::AmemorixPolicy, time, types::*};
use crate::App;
use nous_domain::{
    nousql::{Directive, QueryExpression, QuerySyntax},
    *,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &SearchInput,
    policy: &AmemorixPolicy,
) -> Result<Value> {
    if !policy.permits(
        input.respect_filter,
        &input.chat_id,
        &input.group_id,
        &input.user_id,
    ) {
        return Ok(json!({"success":true,"summary":"","hits":[],"filtered":true}));
    }
    let mut query = QueryFilters {
        person_ids: if input.person_id.is_empty() {
            vec![]
        } else {
            vec![input.person_id.clone()]
        },
        ..Default::default()
    };
    let chats = std::iter::once(&input.chat_id)
        .chain(&input.shared_chat_ids)
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>();
    query.conversations = chats
        .into_iter()
        .map(ConversationRef::new)
        .collect::<Result<_>>()?;
    query.include_global_sources = true;
    if input.respect_filter {
        query.excluded_source_types = policy.excluded_source_types.clone();
    }
    query.validate()?;
    let mut request: QueryRequest = serde_json::from_value(
        json!({"subject_id":binding.subject_id,"input":{"text":"","media":[]},"mode":policy.search_mode,"limit":input.limit,"domains":["cognition"],"filters":query,"time":time::filter(input.time_start.as_ref(),input.time_end.as_ref())?}),
    )?;
    request.expression = Some(match input.mode {
        SearchMode::Search => atom(&input.query, policy.search_mode, None, false)?,
        SearchMode::Time => atom(&input.query, QueryMode::Lexical, None, true)?,
        SearchMode::Hybrid => atom(&input.query, QueryMode::Hybrid, None, false)?,
        SearchMode::Episode => atom(&input.query, policy.search_mode, Some("episode"), false)?,
        SearchMode::Aggregate => QueryExpression::Any {
            children: vec![
                atom(&input.query, policy.search_mode, None, false)?,
                atom("", QueryMode::Lexical, None, true)?,
                atom(&input.query, policy.search_mode, Some("episode"), false)?,
            ],
        },
    });
    let result = app.query(ctx, request).await?;
    let mut hits = Vec::new();
    let mut read = app.database.read_view().await?;
    for hit in &result.hits {
        read.require_source_read(
            ctx,
            &binding.namespace_id,
            &binding.subject_id,
            &hit.reference,
        )
        .await?;
        let mut root_metadata = Vec::new();
        for root in &hit.source_roots {
            root_metadata.push(read.material_details(&binding.namespace_id, root).await?);
        }
        let mut metadata = if root_metadata.len() == 1 {
            root_metadata[0].metadata.clone()
        } else {
            json!({})
        };
        let metadata = metadata
            .as_object_mut()
            .ok_or_else(|| Error::internal("stored material metadata is not an object"))?;
        metadata.insert("source_roots".into(), json!(hit.source_roots));
        metadata.insert("root_metadata".into(), json!(root_metadata));
        metadata.insert("reference".into(), json!(hit.reference));
        let (kind, title, content) = content(&hit.object)?;
        let source = match &hit.object {
            RetrievalRecord::Memory(memory) => read
                .material_by_id(&memory.material_id)
                .await?
                .source
                .unwrap_or_default(),
            _ => String::new(),
        };
        let hash = match &hit.object {
            RetrievalRecord::Memory(memory) => memory.id.to_string(),
            _ => hit.reference.to_string(),
        };
        hits.push(json!({"hash":hash,"type":kind,"title":title,"content":content,"score":hit.score,"source":source,"metadata":metadata,"episode_id":if kind=="episode" {hit.reference.to_string()}else{String::new()}}));
    }
    read.finish().await?;
    let summary = hits
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, hit)| {
            format!(
                "{}. {}",
                index + 1,
                hit["content"]
                    .as_str()
                    .unwrap_or_default()
                    .replace('\n', " ")
                    .chars()
                    .take(120)
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(
        json!({"success":true,"summary":summary,"hits":hits,"filtered":false,"coverage":result.coverage,"semantic":result.semantic,"retrieval_mode":input.mode,"time_start":input.time_start,"time_end":input.time_end}),
    )
}
fn atom(
    text: &str,
    mode: QueryMode,
    kind: Option<&str>,
    time_order: bool,
) -> Result<QueryExpression> {
    let mut directives = vec![Directive {
        name: "mode".into(),
        positional: vec![serde_json::to_value(mode)?],
        named: BTreeMap::new(),
    }];
    if let Some(kind) = kind {
        directives.push(Directive {
            name: "return".into(),
            positional: vec![json!(kind)],
            named: BTreeMap::new(),
        });
    }
    if time_order {
        directives.push(Directive {
            name: "sort".into(),
            positional: vec![json!("occurred"), json!("desc")],
            named: BTreeMap::new(),
        });
    }
    Ok(QueryExpression::Atom {
        source: QuerySyntax {
            intent: text.to_owned(),
            cues: vec![],
            directives,
            preferences: vec![],
        }
        .canonical(),
    })
}
fn content(record: &RetrievalRecord) -> Result<(&'static str, String, String)> {
    Ok(match record {
        RetrievalRecord::Memory(memory) => {
            ("paragraph", memory.title.clone(), memory.content.clone())
        }
        RetrievalRecord::Structured(object) => match &object.body {
            CognitiveContent::Entity(entity) => (
                "entity",
                entity.label.clone(),
                [
                    entity.description.clone(),
                    entity
                        .attributes
                        .iter()
                        .map(|attribute| format!("{}: {}", attribute.predicate, attribute.value))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            ),
            CognitiveContent::Concept(concept) => (
                "concept",
                concept.label.clone(),
                concept.description.clone(),
            ),
            CognitiveContent::Episode(episode) => (
                "episode",
                episode.title.clone(),
                episode.boundary_explanation.clone(),
            ),
            CognitiveContent::Journal(journal) => {
                ("journal", journal.title.clone(), journal.narrative.clone())
            }
            CognitiveContent::Schema(schema) => {
                ("schema", schema.title.clone(), schema.claim.clone())
            }
            CognitiveContent::Association(association) => (
                "relation",
                association.relation.clone(),
                format!(
                    "{} {} {}",
                    association.from, association.relation, association.to
                ),
            ),
            CognitiveContent::MemorySemantics(_) => (
                "memory_semantics",
                String::new(),
                serde_json::to_string(&object.body)?,
            ),
        },
        RetrievalRecord::Media(_) => {
            return Err(Error::internal(
                "A-Memorix cognition query returned a media object",
            ));
        }
    })
}
