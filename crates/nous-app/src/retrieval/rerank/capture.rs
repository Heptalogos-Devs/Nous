use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct AcceptedQuery {
    pub request: QueryRequest,
    pub current_view: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Candidate {
    pub reference: RetrievalTarget,
    pub sha256: String,
    pub sent_sha256: String,
    pub original_bytes: usize,
    pub sent_bytes: usize,
    pub current_head: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct RerankRun {
    pub query: String,
    pub query_sources: Vec<SourceReadRef>,
    pub queries: Vec<AcceptedQuery>,
    pub candidates: Vec<Candidate>,
    pub policy: super::super::config::RetrievalPolicy,
}
pub(super) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
impl App {
    pub(super) async fn rerank_projection(
        &self,
        ctx: &CallContext,
        queries: &[AcceptedQuery],
        policy: &super::super::config::RetrievalPolicy,
    ) -> Result<BTreeMap<RetrievalTarget, (String, bool)>> {
        let mut documents = BTreeMap::<RetrievalTarget, (String, bool)>::new();
        for query in queries {
            let mut request = query.request.clone();
            if query.current_view {
                request.as_of_commit = None;
            }
            let ns = self.database.subject_namespace(&request.subject_id).await?;
            let view = self.query_view(ctx, &ns, &request, "", policy).await?;
            for entry in view.selected(&request) {
                if !entry.sources_complete {
                    continue;
                }
                let Some(text) = &entry.lexical_text else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let current = query.current_view && !request.history && request.reference.is_none();
                documents
                    .entry(entry.reference())
                    .and_modify(|(_, all_current)| *all_current &= current)
                    .or_insert_with(|| (text.clone(), current));
            }
        }
        Ok(documents)
    }
    pub(super) async fn capture_rerank(
        &self,
        ctx: &CallContext,
        plan: &nous_domain::nousql::QueryPlan,
        policy: &super::super::config::RetrievalPolicy,
        hits: &[QueryHit],
    ) -> Result<RerankRun> {
        let queries = plan
            .leaves
            .iter()
            .filter(|leaf| !leaf.empty)
            .map(|leaf| AcceptedQuery {
                request: leaf.request.clone(),
                current_view: leaf.current_view,
            })
            .collect::<Vec<_>>();
        let documents = self.rerank_projection(ctx, &queries, policy).await?;
        let mut intents = BTreeSet::new();
        for query in &queries {
            let text = query.request.input.text.trim();
            if !text.is_empty() {
                intents.insert(text);
            }
        }
        let query = intents.into_iter().collect::<Vec<_>>().join("\n");
        let mut remaining = policy.rerank.max_input_bytes.saturating_sub(query.len());
        let mut candidates = Vec::new();
        for hit in hits {
            if candidates.len() == policy.rerank.max_candidates {
                break;
            }
            let Some((text, current_head)) = documents.get(&hit.reference) else {
                continue;
            };
            let sent = prefix(text, policy.rerank.max_document_bytes.min(remaining));
            if sent.trim().is_empty() {
                continue;
            }
            candidates.push(Candidate {
                reference: hit.reference.clone(),
                sha256: digest(text),
                sent_sha256: digest(sent),
                original_bytes: text.len(),
                sent_bytes: sent.len(),
                current_head: *current_head,
            });
            remaining -= sent.len();
        }
        let mut query_sources = plan
            .leaves
            .iter()
            .flat_map(|leaf| leaf.query_sources.clone())
            .collect::<Vec<_>>();
        query_sources.sort();
        query_sources.dedup();
        Ok(RerankRun {
            query,
            query_sources,
            queries,
            candidates,
            policy: policy.clone(),
        })
    }
    pub(super) async fn rerank_input(
        &self,
        ctx: &CallContext,
        run: &RerankRun,
    ) -> Result<model::RerankInput> {
        let documents = self
            .rerank_projection(ctx, &run.queries, &run.policy)
            .await?;
        let mut input = Vec::new();
        for accepted in &run.candidates {
            let Some((text, _)) = documents.get(&accepted.reference) else {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "rerank candidate is no longer eligible",
                ));
            };
            if digest(text) != accepted.sha256 || text.len() != accepted.original_bytes {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "rerank candidate projection changed",
                ));
            }
            input.push(model::RerankDocument {
                reference: accepted.reference.clone(),
                text: prefix(text, accepted.sent_bytes).into(),
                original_bytes: accepted.original_bytes,
                current_head: accepted.current_head,
            });
        }
        Ok(model::RerankInput {
            query: run.query.clone(),
            top_n: input.len(),
            documents: input,
            query_sources: run.query_sources.clone(),
        })
    }
}
