use anyhow::Result;
use nous_retrieval::*;
use std::collections::BTreeSet;
fn document(id: &str, namespace: &str, subject: &str, seq: i64, text: &str) -> IndexDocument {
    IndexDocument {
        id: id.into(),
        namespace_id: namespace.into(),
        subject_id: subject.into(),
        revision: seq,
        commit_seq: seq,
        text: text.into(),
        deleted: false,
    }
}

#[test]
fn chinese_scope_incremental_deletion_restart_and_rebuild() -> Result<()> {
    let root = tempfile::tempdir()?;
    let index = RetrievalIndex::open(root.path())?;
    index.apply(
        &[
            document(
                "a",
                "ns-a",
                "subject-a",
                1,
                "北京大学研究认知架构和持久记忆",
            ),
            document("b", "ns-b", "subject-b", 2, "北京大学 北京大学 北京大学"),
            document("c", "ns-a", "subject-c", 3, "北京大学 北京大学"),
        ],
        3,
    )?;
    let page = index.search("北京大学的记忆架构", "ns-a", "subject-a", 1)?;
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].id, "a");
    assert_eq!(page.coverage.through_commit_seq, 3);
    assert!(index.search("?!", "ns-a", "subject-a", 10)?.hits.is_empty());
    assert!(match_score("记忆架构", "认知架构和持久记忆").is_some());
    let invalid = document("a", "ns-a", "subject-a", 5, "new");
    assert!(index.apply(&[invalid], 4).is_err());
    assert_eq!(index.coverage()?.through_commit_seq, 3);
    index.apply(
        &[document(
            "a",
            "ns-a",
            "subject-a",
            4,
            "Apple's language model",
        )],
        4,
    )?;
    assert!(
        index
            .search("北京大学", "ns-a", "subject-a", 10)?
            .hits
            .is_empty()
    );
    assert_eq!(
        index
            .search("unknown_field:Apple AND (model", "ns-a", "subject-a", 10)?
            .hits
            .len(),
        1
    );
    let mut deleted = document("a", "ns-a", "subject-a", 5, "");
    deleted.deleted = true;
    index.apply(&[deleted], 5)?;
    assert!(
        index
            .search("Apple", "ns-a", "subject-a", 10)?
            .hits
            .is_empty()
    );
    let generation = index.coverage()?.generation;
    index.rebuild(
        &[document("d", "ns-a", "subject-a", 6, "可追溯的中文记忆")],
        6,
    )?;
    assert_ne!(index.coverage()?.generation, generation);
    index.close()?;
    let reopened = RetrievalIndex::open(root.path())?;
    assert_eq!(reopened.coverage()?.through_commit_seq, 6);
    assert_eq!(reopened.coverage()?.document_count, 1);
    assert_eq!(
        reopened.search("追溯记忆", "ns-a", "subject-a", 10)?.hits[0].id,
        "d"
    );
    drop(reopened);
    assert!(index.apply(&[], 7).is_err());
    Ok(())
}

#[test]
fn scoped_content_reuses_persisted_projection_and_filters_before_top_k() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let index = RetrievalIndex::open(directory.path())?;
    let documents = vec![
        document("memory:a@1", "ns", "subject", 1, "北京记忆"),
        document("concept:b@1", "ns", "subject", 2, "北京 北京 北京记忆"),
    ];
    let coverage = index.sync_scope("ns", "subject", &documents, 2)?;
    let repeated = index.sync_scope("ns", "subject", &documents, 5)?;
    assert_eq!(coverage.generation, repeated.generation);
    let allowed = BTreeSet::from(["memory:a@1".to_owned()]);
    assert_eq!(
        index
            .search_allowed("北京", "ns", "subject", 1, &allowed)?
            .hits[0]
            .id,
        "memory:a@1"
    );
    assert!(
        index
            .search_allowed("北京", "ns", "subject", 1, &BTreeSet::new())?
            .hits
            .is_empty()
    );
    index.close()?;
    let reopened = RetrievalIndex::open(directory.path())?;
    assert_eq!(
        reopened
            .sync_scope("ns", "subject", &documents, 5)?
            .generation,
        coverage.generation
    );
    let historical = vec![document("schema:old@1", "ns", "subject", 1, "北京旧结论")];
    reopened.sync_scope("ns", "subject", &historical, 1)?;
    assert!(
        reopened
            .search_allowed("北京", "ns", "subject", 1, &allowed)?
            .hits
            .is_empty()
    );
    assert_eq!(
        reopened.search("旧结论", "ns", "subject", 1)?.hits[0].id,
        "schema:old@1"
    );
    Ok(())
}
