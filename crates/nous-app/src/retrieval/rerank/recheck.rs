use super::*;
use nous_domain::nousql::{PlannedExpression, QueryPlan};

impl App {
    pub(in crate::retrieval) async fn recheck_reranked_hits(
        &self,
        ctx: &CallContext,
        plan: &QueryPlan,
        policy: &super::super::config::RetrievalPolicy,
        hits: &mut Vec<QueryHit>,
    ) -> Result<(bool, i64)> {
        let mut eligible = Vec::new();
        let mut watermark = plan.authority_commit_seq;
        for leaf in &plan.leaves {
            if leaf.empty {
                eligible.push(BTreeSet::new());
                continue;
            }
            let mut request = leaf.request.clone();
            if leaf.current_view {
                request.as_of_commit = None;
            }
            let ns = self.database.subject_namespace(&request.subject_id).await?;
            let view = self.query_view(ctx, &ns, &request, "", policy).await?;
            watermark = watermark.max(view.watermark);
            eligible.push(
                view.selected(&request)
                    .map(|entry| entry.reference())
                    .collect(),
            );
        }
        let allowed = combine(&plan.expression, &eligible);
        let before = hits.len();
        hits.retain(|hit| allowed.contains(&hit.reference));
        Ok((hits.len() == before, watermark))
    }
}
fn combine(
    expression: &PlannedExpression,
    leaves: &[BTreeSet<RetrievalTarget>],
) -> BTreeSet<RetrievalTarget> {
    match expression {
        PlannedExpression::Leaf { index } => leaves[*index].clone(),
        PlannedExpression::Any { children } => children
            .iter()
            .flat_map(|child| combine(child, leaves))
            .collect(),
        PlannedExpression::All { children } => {
            let mut sets = children.iter().map(|child| combine(child, leaves));
            let mut result = sets.next().unwrap_or_default();
            for set in sets {
                result.retain(|target| set.contains(target));
            }
            result
        }
    }
}
