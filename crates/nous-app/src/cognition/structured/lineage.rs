use super::*;
use nous_storage::WriteTransaction;
use std::collections::BTreeSet;

impl App {
    pub async fn merge_concepts(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: MergeConceptsRequest,
    ) -> Result<OperationReceipt> {
        input.validate()?;
        let namespace = self
            .database
            .structured_namespace(&input.survivor.id)
            .await?;
        let head = self
            .database
            .structured_object(&namespace, &input.survivor.id)
            .await?;
        ctx.require_subject(&namespace, &head.subject_id)?;
        let scope = format!("namespace:{namespace}/subject:{}", head.subject_id);
        let key = operation_key(ctx, "concept.merge", &scope, operation_id, &input)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut survivor = expect_object(
            &mut tx,
            &namespace,
            &head.subject_id,
            &input.survivor,
            CognitiveKind::Concept,
        )
        .await?;
        require_canonical(&survivor)?;
        let survivor_before = survivor.reference();
        let mut retired = Vec::new();
        for expected in &input.retired {
            let object = expect_object(
                &mut tx,
                &namespace,
                &head.subject_id,
                expected,
                CognitiveKind::Concept,
            )
            .await?;
            require_canonical(&object)?;
            retired.push(object);
        }
        let merged_ids = retired
            .iter()
            .map(|object| object.id.to_string())
            .chain(std::iter::once(survivor.id.to_string()))
            .collect::<BTreeSet<_>>();
        let mut merged = concept(&survivor)?.clone();
        let mut alias_keys = merged.alias_keys().into_iter().collect::<BTreeSet<_>>();
        let mut members = merged.members.iter().cloned().collect::<BTreeSet<_>>();
        let mut evidence = survivor.evidence.clone();
        for source in &retired {
            let source_content = concept(source)?;
            for alias in std::iter::once(&source_content.label).chain(&source_content.aliases) {
                if alias_keys.insert(alias.trim().to_lowercase()) {
                    merged.aliases.push(alias.clone());
                }
            }
            members.extend(source_content.members.iter().cloned());
            append_lineage(
                &mut merged.lineage,
                LineageLink {
                    relation: LineageRelation::MergedFrom,
                    target: source.reference(),
                    reason: input.reason.clone(),
                },
            );
            evidence.extend(source.evidence.iter().cloned());
        }
        // Retired Concept identities become aliases, rather than self-members of
        // their survivor. Referenced Memory revisions and other members stay exact.
        merged.members = members
            .into_iter()
            .filter(|reference| {
                reference.kind != CognitiveKind::Concept || !merged_ids.contains(&reference.id)
            })
            .collect();
        evidence.extend(input.evidence.iter().cloned());
        // Release the retired aliases first. Each redirect names the survivor's
        // immutable pre-merge revision; the survivor lineage names old source refs.
        for source in &mut retired {
            let expected = source.revision;
            let CognitiveContent::Concept(content) = &mut source.body else {
                return Err(Error::invalid("merge source is not Concept"));
            };
            content.canonical = Some(survivor_before.clone());
            source.evidence = merged_evidence(
                source
                    .evidence
                    .iter()
                    .cloned()
                    .chain(input.evidence.iter().cloned()),
            );
            revise_stamp(source, &tx)?;
            tx.save_structured(source, Some(expected)).await?;
        }
        let expected = survivor.revision;
        survivor.body = CognitiveContent::Concept(merged);
        survivor.evidence = merged_evidence(evidence);
        revise_stamp(&mut survivor, &tx)?;
        tx.save_structured(&survivor, Some(expected)).await?;
        tx.commit(json!({"survivor":survivor,"retired":retired}))
            .await
    }

    /// Split preserves the broader parent, matching the mature Tag split operation.
    pub async fn split_concept(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: SplitConceptRequest,
    ) -> Result<OperationReceipt> {
        input.validate()?;
        let namespace = self.database.structured_namespace(&input.parent.id).await?;
        let head = self
            .database
            .structured_object(&namespace, &input.parent.id)
            .await?;
        ctx.require_subject(&namespace, &head.subject_id)?;
        let scope = format!("namespace:{namespace}/subject:{}", head.subject_id);
        let key = operation_key(ctx, "concept.split", &scope, operation_id, &input)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let parent = expect_object(
            &mut tx,
            &namespace,
            &head.subject_id,
            &input.parent,
            CognitiveKind::Concept,
        )
        .await?;
        require_canonical(&parent)?;
        let mut children = Vec::new();
        for mut content in input.children {
            append_lineage(
                &mut content.lineage,
                LineageLink {
                    relation: LineageRelation::SplitFrom,
                    target: parent.reference(),
                    reason: input.reason.clone(),
                },
            );
            let evidence = merged_evidence(
                parent
                    .evidence
                    .iter()
                    .cloned()
                    .chain(input.evidence.iter().cloned()),
            );
            let child = new_object(&parent, CognitiveContent::Concept(content), evidence, &tx);
            tx.save_structured(&child, None).await?;
            children.push(child);
        }
        tx.commit(json!({"parent":parent,"children":children}))
            .await
    }

    pub async fn resegment_episodes(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        input: ResegmentEpisodesRequest,
    ) -> Result<OperationReceipt> {
        input.validate()?;
        let namespace = self
            .database
            .structured_namespace(&input.sources[0].id)
            .await?;
        let head = self
            .database
            .structured_object(&namespace, &input.sources[0].id)
            .await?;
        ctx.require_subject(&namespace, &head.subject_id)?;
        let scope = format!("namespace:{namespace}/subject:{}", head.subject_id);
        let key = operation_key(ctx, "episode.resegment", &scope, operation_id, &input)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut sources = Vec::new();
        for expected in &input.sources {
            sources.push(
                expect_object(
                    &mut tx,
                    &namespace,
                    &head.subject_id,
                    expected,
                    CognitiveKind::Episode,
                )
                .await?,
            );
        }
        sources.sort_by_key(|object| {
            let content = episode(object).expect("expected Episode type");
            (
                content.experience.start.unwrap_or(i64::MIN),
                content.experience.end.unwrap_or(i64::MAX),
                object.id.clone(),
            )
        });
        validate_resegmentation(&sources, &input.segments)?;
        let source_snapshot = sources.clone();
        // A one-to-one reinterpretation keeps the stable Episode identity.
        if sources.len() == 1 && input.segments.len() == 1 {
            let mut object = sources.remove(0);
            let expected = object.revision;
            let content = input
                .segments
                .into_iter()
                .next()
                .expect("validated one segment");
            let evidence = merged_evidence(object.evidence.iter().cloned().chain(input.evidence));
            let unchanged = serde_json::to_value(&object.body)?
                == serde_json::to_value(CognitiveContent::Episode(content.clone()))?
                && serde_json::to_value(&object.evidence)? == serde_json::to_value(&evidence)?;
            if !unchanged {
                object.body = CognitiveContent::Episode(content);
                object.evidence = evidence;
                revise_stamp(&mut object, &tx)?;
                tx.save_structured(&object, Some(expected)).await?;
            }
            return tx.commit(json!({"retired":[],"episodes":[object]})).await;
        }
        // Retire source organization before publishing replacement siblings, so
        // parent/range checks see the whole new partition in one write transaction.
        for source in &mut sources {
            let expected = source.revision;
            source.deleted = true;
            revise_stamp(source, &tx)?;
            tx.save_structured(source, Some(expected)).await?;
        }
        let mut episodes = Vec::new();
        for mut segment in input.segments {
            let member_sources = segment
                .members
                .iter()
                .map(|member| &member.source)
                .collect::<BTreeSet<_>>();
            let mut evidence = input.evidence.clone();
            for source in &source_snapshot {
                if episode(source)?
                    .members
                    .iter()
                    .any(|member| member_sources.contains(&member.source))
                {
                    append_lineage(
                        &mut segment.lineage,
                        LineageLink {
                            relation: LineageRelation::ResegmentedFrom,
                            target: source.reference(),
                            reason: input.reason.clone(),
                        },
                    );
                    evidence.extend(source.evidence.iter().cloned());
                }
            }
            let object = new_object(
                &source_snapshot[0],
                CognitiveContent::Episode(segment),
                merged_evidence(evidence),
                &tx,
            );
            tx.save_structured(&object, None).await?;
            episodes.push(object);
        }
        tx.commit(json!({"retired":sources,"episodes":episodes}))
            .await
    }
}

async fn expect_object(
    tx: &mut WriteTransaction,
    namespace: &NamespaceId,
    subject: &SubjectId,
    target: &StructuredExpectation,
    kind: CognitiveKind,
) -> Result<StructuredObject> {
    let object = tx.structured_object(namespace, &target.id).await?;
    if object.subject_id != *subject
        || object.kind() != kind
        || object.revision != target.expected_revision
        || object.deleted
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "structured maintenance source scope, kind, head or state changed",
        ));
    }
    Ok(object)
}
fn concept(object: &StructuredObject) -> Result<&ConceptContent> {
    match &object.body {
        CognitiveContent::Concept(content) => Ok(content),
        _ => Err(Error::invalid("operation requires Concept")),
    }
}
fn require_canonical(object: &StructuredObject) -> Result<()> {
    if concept(object)?.canonical.is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "operation requires a canonical Concept head",
        ));
    }
    Ok(())
}
fn episode(object: &StructuredObject) -> Result<&EpisodeContent> {
    match &object.body {
        CognitiveContent::Episode(content) => Ok(content),
        _ => Err(Error::invalid("operation requires Episode")),
    }
}
fn revise_stamp(object: &mut StructuredObject, tx: &WriteTransaction) -> Result<()> {
    object.revision = object
        .revision
        .checked_add(1)
        .ok_or_else(|| Error::invalid("revision overflow"))?;
    object.times.formation = tx.time().point();
    object.times.recorded = tx.time().point();
    object.recorded_at = tx.time().recorded();
    object.commit_seq = tx.commit_seq();
    object.producer_call_id = None;
    Ok(())
}
fn new_object(
    source: &StructuredObject,
    body: CognitiveContent,
    evidence: Vec<EvidenceLink>,
    tx: &WriteTransaction,
) -> StructuredObject {
    let mut times = source.times.clone();
    times.formation = tx.time().point();
    times.recorded = tx.time().point();
    StructuredObject {
        id: CognitiveId::new(),
        namespace_id: source.namespace_id.clone(),
        subject_id: source.subject_id.clone(),
        revision: 1,
        body,
        evidence,
        times,
        producer_call_id: None,
        recorded_at: tx.time().recorded(),
        commit_seq: tx.commit_seq(),
        deleted: false,
    }
}
fn merged_evidence(links: impl IntoIterator<Item = EvidenceLink>) -> Vec<EvidenceLink> {
    let mut seen = BTreeSet::new();
    links
        .into_iter()
        .filter(|link| seen.insert((link.source.clone(), link.polarity, link.relation)))
        .collect()
}
fn append_lineage(links: &mut Vec<LineageLink>, link: LineageLink) {
    if !links
        .iter()
        .any(|old| old.target == link.target && old.relation == link.relation)
    {
        links.push(link);
    }
}
fn validate_resegmentation(
    sources: &[StructuredObject],
    segments: &[EpisodeContent],
) -> Result<()> {
    let first = episode(&sources[0])?;
    let mut expected = Vec::new();
    let mut seen = BTreeSet::new();
    let mut lower = first.experience.start;
    let mut upper = first.experience.end;
    for source in sources {
        let content = episode(source)?;
        if content.track != first.track
            || content.parent != first.parent
            || content.experience.domain != first.experience.domain
        {
            return Err(Error::invalid(
                "resegmentation sources must share track, parent revision and time domain",
            ));
        }
        lower = match (lower, content.experience.start) {
            (Some(a), Some(b)) => Some(a.min(b)),
            _ => None,
        };
        upper = match (upper, content.experience.end) {
            (Some(a), Some(b)) => Some(a.max(b)),
            _ => None,
        };
        for member in &content.members {
            if seen.insert(member.source.clone()) {
                expected.push(member.source.clone());
            }
        }
    }
    let supplied = segments
        .iter()
        .flat_map(|segment| segment.members.iter().map(|member| member.source.clone()))
        .collect::<Vec<_>>();
    if supplied != expected {
        return Err(Error::invalid(
            "resegmentation must retain every source member exactly once in original order",
        ));
    }
    for segment in segments {
        if segment.track != first.track
            || segment.parent != first.parent
            || segment.experience.domain != first.experience.domain
            || lower.is_some_and(|bound| segment.experience.start.is_none_or(|start| start < bound))
            || upper.is_some_and(|bound| segment.experience.end.is_none_or(|end| end > bound))
        {
            return Err(Error::invalid(
                "replacement segment changed track/parent or escaped the source experience span",
            ));
        }
    }
    for pair in segments.windows(2) {
        if matches!((pair[0].experience.end,pair[1].experience.start),(Some(end),Some(start))if end>start)
        {
            return Err(Error::invalid(
                "replacement segments overlap or reverse experience order",
            ));
        }
    }
    Ok(())
}
