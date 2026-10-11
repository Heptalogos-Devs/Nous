// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveQuery {
    #[serde(default)]
    pub projection: ResultProjection,
    #[serde(default)]
    pub temporal_frame: TemporalFrame,
    #[serde(default)]
    pub work_context: Option<uuid::Uuid>,
    #[serde(default)]
    pub subject: SubjectId,
    pub session: Option<SessionId>,
    #[serde(default)]
    pub situation: SituationDescriptor,
    pub expression: CognitiveQueryExpr,
    #[serde(default)]
    pub exploration: ExplorationIntent,
    #[serde(default)]
    pub resources: ResourceIntent,
    #[serde(default)]
    pub result_need: ResultNeed,
    #[serde(default)]
    pub effort: CognitiveEffort,
    #[serde(default)]
    pub capabilities: CapabilityPolicy,
    #[serde(default)]
    pub diagnostics: DiagnosticsRequest,
}

impl CognitiveQuery {
    /// Exact targets close the candidate set within their expression scope.
    /// Child scopes inherit targets unless they supply their own, just as in
    /// Runtime tree execution. A mixed tree still permits discovery elsewhere.
    pub fn is_exact_read(&self) -> bool {
        fn exact(node: &CognitiveQueryExpr, inherited: &[QueryTarget]) -> bool {
            let targets = if node.targets.is_empty() {
                inherited
            } else {
                &node.targets
            };
            if node.operation == QueryOperation::Atom {
                targets
                    .iter()
                    .any(|target| matches!(target, QueryTarget::Exact { .. }))
            } else {
                !node.children.is_empty() && node.children.iter().all(|child| exact(child, targets))
            }
        }
        exact(&self.expression, &[])
    }
    pub fn requests_resources(&self) -> bool {
        if self.is_exact_read() {
            return false;
        }
        self.projection.domains.contains(&ResultDomain::Resource)
            || self.resources.synopsis_only
            || self.exploration == ExplorationIntent::Global
            || self.scopes().iter().any(|scope| {
                scope.constraints.current_authority != CurrentAuthorityNeed::None
                    || scope.cues.iter().any(|cue| matches!(cue, Cue::Resource(_)))
            })
    }
    pub fn scopes(&self) -> Vec<&CognitiveQueryExpr> {
        let mut pending = vec![&self.expression];
        let mut scopes = Vec::new();
        while let Some(node) = pending.pop() {
            scopes.push(node);
            pending.extend(node.children.iter().rev());
        }
        scopes
    }
    pub fn validate(&self) -> Result<()> {
        self.projection.validate()?;
        if self.result_need.limit == 0 || self.result_need.limit > MAX_QUERY_RESULT_ITEMS {
            return Err(Error::new(
                crate::ErrorOwner::Core,
                crate::ErrorCode::InvalidRequest,
                "query result limit must be between 1 and 2048",
            ));
        }
        let mut nodes = vec![(&self.expression, 0)];
        let mut count = 0;
        while let Some((node, depth)) = nodes.pop() {
            count += 1;
            if count > 64 {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "query expression exceeds the 64-node limit",
                ));
            }
            if depth > 16 {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "query expression exceeds the 16-level depth limit",
                ));
            }
            if node.cues.len() > 256 {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "query expression node exceeds the 256-cue limit",
                ));
            }
            if node.targets.len() > 128 {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "query expression node exceeds the 128-target limit",
                ));
            }
            if node.preferences.len() > 16 {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "query expression node exceeds the 16-preference limit",
                ));
            }
            if (node.operation == QueryOperation::Atom && !node.children.is_empty())
                || (node.operation != QueryOperation::Atom
                    && (node.children.len() < 2 || !node.cues.is_empty()))
            {
                return Err(Error::new(
                    crate::ErrorOwner::Core,
                    crate::ErrorCode::InvalidRequest,
                    "invalid query expression shape",
                ));
            }
            for interval in [
                node.constraints.occurred,
                node.constraints.observed,
                node.constraints.valid,
                node.constraints.formed,
                node.constraints.recorded,
            ]
            .into_iter()
            .flatten()
            {
                interval.validate()?;
            }
            nodes.extend(node.children.iter().map(|child| (child, depth + 1)));
        }
        if self.capabilities.text_embedding == RequirementStrength::Forbidden
            && self.capabilities.residual_sensing == RequirementStrength::Required
        {
            return Err(Error::new(
                crate::ErrorOwner::Core,
                crate::ErrorCode::InvalidRequest,
                "Residual sensing cannot be required when text embedding is forbidden",
            ));
        }
        Ok(())
    }
}
