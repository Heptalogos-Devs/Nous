// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl SelfService {
    pub async fn project_as_of(
        &self,
        subject: SubjectId,
        at: DateTime<Utc>,
        revision_view: RevisionView,
    ) -> Result<Vec<HistoricalCognitionState>> {
        self.require_enabled(subject).await?;
        let rows=sqlx::query("SELECT o.object_id,o.family,h.state,ARRAY(SELECT r.revision_id FROM self_revisions r WHERE r.object_id=o.object_id AND r.recorded_at<=$2 ORDER BY r.recorded_at DESC,r.revision_id DESC) revisions FROM self_objects o JOIN LATERAL (SELECT state FROM authority_object_states WHERE subject_id=$1 AND object_kind='self' AND object_ref=o.object_id::text AND recorded_at<=$2 ORDER BY recorded_at DESC,event_id DESC LIMIT 1) h ON true WHERE o.subject_id=$1 AND o.created_at<=$2 ORDER BY o.object_id").bind(subject.0).bind(at).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = Vec::new();
        for row in rows {
            let object: Uuid = row.try_get("object_id").map_err(db)?;
            let family: String = row.try_get("family").map_err(db)?;
            let ids: Vec<Uuid> = row.try_get("revisions").map_err(db)?;
            if ids.is_empty() {
                continue;
            }
            let recorded_revisions = ids
                .into_iter()
                .map(|id| parse_reference(&format!("{family}_revision"), &id.to_string()))
                .collect::<Result<Vec<_>>>()?;
            let head = recorded_revisions[0].clone();
            let revisions = if revision_view == RevisionView::Current {
                vec![head.clone()]
            } else {
                recorded_revisions.clone()
            };
            result.push(HistoricalCognitionState {
                object: parse_reference(&family, &object.to_string())?,
                head,
                revisions,
                recorded_revisions,
                state: row.try_get("state").map_err(db)?,
            });
        }
        Ok(result)
    }
}
