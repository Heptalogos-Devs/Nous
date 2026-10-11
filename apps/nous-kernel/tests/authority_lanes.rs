// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, Utc};
use nous_core::{CognitiveRef, Cue, EntityRef, TimePredicate};
use test_support::query::{query, subject};
use test_support::{database, open_runtime};
use uuid::Uuid;

#[tokio::test]
async fn authority_lanes_reach_matches_beyond_first_n_objects() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let target_entity = "entity:tail-match";
    let temporal_start = chrono::SubsecRound::trunc_subsecs(Utc::now(), 6) - Duration::hours(1);
    let temporal_end = temporal_start + Duration::minutes(30);
    let mut tx = runtime.store.begin().await.expect("transaction");
    let mut tail_revision = None;
    for index in 0..10_000u32 {
        let memory_id = Uuid::now_v7();
        let revision_id = Uuid::now_v7();
        let valid = index == 9_999;
        sqlx::query("INSERT INTO memory_objects(memory_id,subject_id,cognitive_role,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,accessibility_mode,created_at) VALUES($1,$2,'declarative',$3,1,'accepted','valid','normal','normal','auto',now())")
            .bind(memory_id)
            .bind(subject.0)
            .bind(revision_id)
            .execute(&mut *tx)
            .await
            .expect("memory object");
        sqlx::query("INSERT INTO memory_revisions(memory_revision_id,memory_id,subject_id,revision_no,parent_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end,formed_at,recorded_at) VALUES($1,$2,$3,1,NULL,NULL,'synthesized',NULL,'fact','tail fixture',$4,'inferred',$5,$6,$7,now(),now())")
            .bind(revision_id)
            .bind(memory_id)
            .bind(subject.0)
            .bind(if valid { "tail temporal match" } else { "ordinary fixture" })
            .bind(if valid { "interval" } else { "unknown" })
            .bind(valid.then_some(temporal_start))
            .bind(valid.then_some(temporal_end))
            .execute(&mut *tx)
            .await
            .expect("memory revision");
        if valid {
            sqlx::query("INSERT INTO memory_revision_aboutness(memory_revision_id,entity_ref) VALUES($1,$2)")
                .bind(revision_id)
                .bind(target_entity)
                .execute(&mut *tx)
                .await
                .expect("tail aboutness");
            tail_revision = Some(revision_id);
        }
    }
    tx.commit().await.expect("fixture commit");
    let tail_revision = tail_revision.expect("tail revision");
    let mut entity_query = query(subject);
    entity_query.expression.cues = vec![
        Cue::Text(nous_core::TextCue {
            text: "Recall cognition for this entity".into(),
        }),
        Cue::Entity(nous_core::EntityCue {
            entity_ref: EntityRef::new(target_entity).expect("entity"),
        }),
    ];
    let entity_result = runtime.query(entity_query).await.expect("entity query");
    assert!(entity_result.results.iter().any(|hit| {
        hit.reference == CognitiveRef::MemoryRevision(nous_core::MemoryRevisionId(tail_revision))
    }));
    let mut temporal_query = query(subject);
    temporal_query.expression.constraints.valid = Some(nous_core::TimePredicate::Range {
        start: Some(temporal_start),
        end: Some(temporal_end),
    });
    let temporal_result = runtime.query(temporal_query).await.expect("temporal query");
    assert!(temporal_result.results.iter().any(|hit| {
        hit.reference == CognitiveRef::MemoryRevision(nous_core::MemoryRevisionId(tail_revision))
    }));
    for (at, expected) in [(temporal_start, true), (temporal_end, false)] {
        let mut point = query(subject);
        point.expression.constraints.valid = Some(TimePredicate::Point { at });
        let result = runtime.query(point).await.expect("point temporal lane");
        assert_eq!(
            result.results.iter().any(|hit| hit.reference
                == CognitiveRef::MemoryRevision(nous_core::MemoryRevisionId(tail_revision))),
            expected
        );
    }
}
