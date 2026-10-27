use super::inventory::hidden;
use crate::{ReadView, storage_error};
use nous_domain::*;
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};

impl ReadView {
    pub async fn subject_vector_diagnostics(
        &mut self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<VectorSpaceDiagnostics>> {
        ctx.require_subject(namespace, subject)?;
        let mut by_space = BTreeMap::<String, VectorSpaceDiagnostics>::new();
        let mut contents = BTreeMap::<String, BTreeSet<String>>::new();
        let mut input_states = BTreeSet::new();
        let mut pending = BTreeMap::<String, BTreeSet<(String, String)>>::new();
        let rows=sqlx::query("SELECT s.space_id,s.source_hash,s.source_key,s.target,s.parts,s.read_requirements,s.retired_commit_seq,EXISTS(SELECT 1 FROM embedding_records r WHERE r.namespace_id=s.namespace_id AND r.subject_id=s.subject_id AND r.space_id=s.space_id AND r.source_hash=s.source_hash AND r.source_key=s.source_key AND r.accepted_commit_seq=s.accepted_commit_seq) AS accepted,EXISTS(SELECT 1 FROM embedding_records r WHERE r.namespace_id=s.namespace_id AND r.subject_id=s.subject_id AND r.space_id=s.space_id AND r.source_hash=s.source_hash AND r.source_key=s.source_key) AS available,j.state AS work_state FROM embedding_subscriptions s LEFT JOIN embedding_content_claims c ON c.namespace_id=s.namespace_id AND c.subject_id=s.subject_id AND c.space_id=s.space_id AND c.source_hash=s.source_hash LEFT JOIN task_records j ON j.job_id=c.job_id WHERE s.namespace_id=? AND s.subject_id=? ORDER BY s.space_id,s.source_key,s.accepted_commit_seq")
            .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in rows {
            let input = EmbeddingInputItem {
                target: serde_json::from_str(row.get("target"))?,
                parts: serde_json::from_str(row.get("parts"))?,
                read_requirements: serde_json::from_str(row.get("read_requirements"))?,
            };
            match self
                .require_source_requirements(ctx, namespace, subject, &input.source_requirements())
                .await
            {
                Ok(()) => {}
                Err(error) if hidden(&error) => continue,
                Err(error) => return Err(error),
            }
            let space: String = row.get("space_id");
            if let std::collections::btree_map::Entry::Vacant(entry) = by_space.entry(space.clone())
            {
                entry.insert(VectorSpaceDiagnostics {
                    space: crate::vector::embedding_space_in(&mut self.tx, &space).await?,
                    contents: 0,
                    associations: 0,
                    retired_associations: 0,
                    pending_inputs: 0,
                    input_states: BTreeMap::new(),
                });
            }
            let stats = by_space.get_mut(&space).expect("registered space");
            let accepted: bool = row.get("accepted");
            let retired = row.get::<Option<i64>, _>("retired_commit_seq").is_some();
            let hash: String = row.get("source_hash");
            if !retired
                && input_states.insert((
                    space.clone(),
                    row.get::<String, _>("source_key"),
                    hash.clone(),
                ))
            {
                let kind = match &input.target {
                    EmbeddingTarget::Item(RetrievalTarget::Cognition(reference)) => {
                        crate::codec::enum_name(&reference.kind)?
                    }
                    EmbeddingTarget::Item(RetrievalTarget::Media(_)) => "media".into(),
                    EmbeddingTarget::Tag => "tag".into(),
                    EmbeddingTarget::Query => "query".into(),
                };
                let state = if row.get::<bool, _>("available") {
                    "ready".into()
                } else {
                    row.get::<Option<String>, _>("work_state")
                        .unwrap_or_else(|| "unassigned".into())
                };
                *stats
                    .input_states
                    .entry(kind)
                    .or_default()
                    .entry(state)
                    .or_default() += 1;
            }
            if accepted {
                stats.associations += 1;
                if retired {
                    stats.retired_associations += 1;
                }
                contents.entry(space).or_default().insert(hash);
            } else if !retired && !row.get::<bool, _>("available") {
                pending
                    .entry(space)
                    .or_default()
                    .insert((row.get("source_key"), hash));
            }
        }
        if ctx.source_read.is_empty() {
            let rows=sqlx::query("SELECT space_id,source_hash FROM embedding_contents WHERE namespace_id=? AND subject_id=? ORDER BY space_id,source_hash")
                .bind(namespace.as_str()).bind(subject.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
            for row in rows {
                let space: String = row.get("space_id");
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    by_space.entry(space.clone())
                {
                    entry.insert(VectorSpaceDiagnostics {
                        space: crate::vector::embedding_space_in(&mut self.tx, &space).await?,
                        contents: 0,
                        associations: 0,
                        retired_associations: 0,
                        pending_inputs: 0,
                        input_states: BTreeMap::new(),
                    });
                }
                contents
                    .entry(space)
                    .or_default()
                    .insert(row.get("source_hash"));
            }
        }
        for (id, stats) in &mut by_space {
            stats.contents = contents.get(id).map_or(0, |values| values.len() as u64);
            stats.pending_inputs = pending.get(id).map_or(0, |values| values.len() as u64);
        }
        Ok(by_space.into_values().collect())
    }
}
