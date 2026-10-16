// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;
#[derive(Debug, Serialize)]
pub struct InverseRelationView {
    pub role: String,
    pub record: SocialRecord,
}
#[derive(Debug, Serialize)]
pub struct RelationDirection {
    pub from: EntityRef,
    pub to: EntityRef,
    pub records: Vec<SocialRecord>,
    pub unknown: bool,
    pub inverse_views: Vec<InverseRelationView>,
}
#[derive(Debug, Serialize)]
pub struct RelationPair {
    pub forward: RelationDirection,
    pub reverse: RelationDirection,
    pub omitted: bool,
    pub next: Option<Uuid>,
}
#[derive(Debug, Serialize)]
pub struct ConventionResolution {
    pub original: String,
    pub consumption: String,
    pub candidates: Vec<SocialRecord>,
    pub conflicts: bool,
    pub omitted: bool,
    pub next: Option<Uuid>,
    pub explanation: String,
}
impl SocialService {
    pub async fn pair(
        &self,
        subject: SubjectId,
        from: EntityRef,
        to: EntityRef,
        input: SocialSelectionRequest<'_>,
    ) -> Result<RelationPair> {
        let selected = self
            .select(
                subject,
                SocialSelectionRequest {
                    endpoint: Some(&from),
                    counterpart: Some(&to),
                    ..input
                },
            )
            .await?;
        let mut forward = Vec::new();
        let mut reverse = Vec::new();
        let mut forward_inverse = Vec::new();
        let mut reverse_inverse = Vec::new();
        for r in selected.records {
            let SocialContent::SocialRelation(v) = &r.content else {
                continue;
            };
            if v.from == from && v.to == to {
                forward.push(r.clone());
                if v.symmetric {
                    reverse.push(r);
                } else if let Some(role) = v.relation_type.definition().inverse_read_role {
                    reverse_inverse.push(InverseRelationView {
                        role: role.into(),
                        record: r,
                    });
                }
            } else if v.from == to && v.to == from {
                reverse.push(r.clone());
                if v.symmetric {
                    forward.push(r);
                } else if let Some(role) = v.relation_type.definition().inverse_read_role {
                    forward_inverse.push(InverseRelationView {
                        role: role.into(),
                        record: r,
                    });
                }
            }
        }
        Ok(RelationPair {
            forward: RelationDirection {
                from: from.clone(),
                to: to.clone(),
                unknown: forward.is_empty() && forward_inverse.is_empty() && !selected.omitted,
                inverse_views: forward_inverse,
                records: forward,
            },
            reverse: RelationDirection {
                from: to,
                to: from,
                unknown: reverse.is_empty() && reverse_inverse.is_empty() && !selected.omitted,
                inverse_views: reverse_inverse,
                records: reverse,
            },
            omitted: selected.omitted,
            next: selected.next,
        })
    }
    pub async fn resolve_conventions(
        &self,
        subject: SubjectId,
        original: &str,
        consumption: &str,
        input: SocialSelectionRequest<'_>,
    ) -> Result<ConventionResolution> {
        if !["understanding", "expression"].contains(&consumption)
            || original.is_empty()
            || original.len() > 65536
        {
            return Err(Error::Invalid("Convention resolution needs original utterance and understanding/expression intent".into()));
        }
        let normalized: String = original.nfc().collect();
        let selected = self
            .select(
                subject,
                SocialSelectionRequest {
                    expression: Some(&normalized),
                    ..input
                },
            )
            .await?;
        let meanings = selected
            .records
            .iter()
            .filter_map(|r| {
                if let SocialContent::LanguageConvention(v) = &r.content {
                    Some(v.meaning.as_str())
                } else {
                    None
                }
            })
            .collect::<std::collections::HashSet<_>>();
        Ok(ConventionResolution{original:original.into(),consumption:consumption.into(),conflicts:meanings.len()>1,candidates:selected.records,omitted:selected.omitted,next:selected.next,explanation:if consumption=="expression"{"Use, explain or choose ordinary wording for these actual recipients; hypotheses do not establish shared background."}else{"Literal and explicitly declared variant candidates within authorized current scope; preserve the original wording and unresolved alternatives."}.into()})
    }
}
