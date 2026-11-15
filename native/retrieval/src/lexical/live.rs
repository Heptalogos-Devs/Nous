//! Live BM25 statistics for immutable snapshots with deleted Tantivy documents.
//!
//! Tantivy's default statistics include deleted postings until segments merge.  The
//! snapshot stores aggregate live statistics. The immutable input retains old text,
//! so replacement and deletion only tokenize changed documents.
use super::{IndexDocument, analyzer};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};
use tantivy::{
    Term,
    query::{Bm25StatisticsProvider, BooleanQuery, EnableScoring, Query, Weight},
    schema::Field,
    tokenizer::TokenStream,
};

const FILE: &str = "live-stats.msgpack";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FieldDocument {
    tokens: u64,
    terms: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Document {
    body: FieldDocument,
    title: FieldDocument,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct LiveStats {
    documents: u64,
    body_tokens: u64,
    title_tokens: u64,
    body_frequency: BTreeMap<String, u64>,
    title_frequency: BTreeMap<String, u64>,
}

fn field_document(text: &str) -> FieldDocument {
    let mut tokenizer = analyzer();
    let mut stream = tokenizer.token_stream(text);
    let mut field = FieldDocument::default();
    while stream.advance() {
        field.tokens += 1;
        field.terms.insert(stream.token().text.clone());
    }
    field
}

fn subtract_field(
    total: &mut u64,
    counts: &mut BTreeMap<String, u64>,
    field: &FieldDocument,
) -> Result<()> {
    *total = total
        .checked_sub(field.tokens)
        .context("invalid live lexical token count")?;
    for term in &field.terms {
        let count = counts
            .get_mut(term)
            .context("invalid live lexical document frequency")?;
        *count = count
            .checked_sub(1)
            .context("invalid live lexical document frequency")?;
        if *count == 0 {
            counts.remove(term);
        }
    }
    Ok(())
}

fn add_field(total: &mut u64, counts: &mut BTreeMap<String, u64>, field: &FieldDocument) {
    *total += field.tokens;
    for term in &field.terms {
        *counts.entry(term.clone()).or_default() += 1;
    }
}

impl LiveStats {
    pub(crate) fn load(path: &Path) -> Result<Option<Self>> {
        let path = path.join(FILE);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(rmp_serde::from_slice(&std::fs::read(path)?)?))
    }

    pub(crate) fn count(&self) -> usize {
        self.documents as usize
    }

    pub(crate) fn remove(&mut self, document: &IndexDocument) -> Result<()> {
        let old = Document {
            body: field_document(&document.text),
            title: field_document(&document.title),
        };
        self.documents = self
            .documents
            .checked_sub(1)
            .context("invalid live lexical document count")?;
        subtract_field(&mut self.body_tokens, &mut self.body_frequency, &old.body)?;
        subtract_field(
            &mut self.title_tokens,
            &mut self.title_frequency,
            &old.title,
        )?;
        Ok(())
    }

    pub(crate) fn insert(&mut self, document: &IndexDocument) {
        let next = Document {
            body: field_document(&document.text),
            title: field_document(&document.title),
        };
        add_field(&mut self.body_tokens, &mut self.body_frequency, &next.body);
        add_field(
            &mut self.title_tokens,
            &mut self.title_frequency,
            &next.title,
        );
        self.documents += 1;
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path.join(FILE))?;
        file.write_all(&rmp_serde::to_vec_named(self)?)?;
        file.sync_all()?;
        Ok(())
    }
}

struct Statistics<'a> {
    data: &'a LiveStats,
    body: Field,
    title: Field,
}
impl Bm25StatisticsProvider for Statistics<'_> {
    fn total_num_tokens(&self, field: Field) -> tantivy::Result<u64> {
        Ok(if field == self.body {
            self.data.body_tokens
        } else if field == self.title {
            self.data.title_tokens
        } else {
            0
        })
    }
    fn total_num_docs(&self) -> tantivy::Result<u64> {
        Ok(self.data.documents)
    }
    fn doc_freq(&self, term: &Term) -> tantivy::Result<u64> {
        let counts = if term.field() == self.body {
            &self.data.body_frequency
        } else {
            &self.data.title_frequency
        };
        Ok(term
            .value()
            .as_str()
            .and_then(|text| counts.get(text))
            .copied()
            .unwrap_or(0))
    }
}

#[derive(Clone, Debug)]
pub(super) struct LiveQuery {
    pub query: BooleanQuery,
    pub data: Arc<LiveStats>,
    pub body: Field,
    pub title: Field,
}
impl Query for LiveQuery {
    fn weight(&self, enabled: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
        if enabled.is_scoring_enabled() {
            let searcher = enabled.searcher().expect("enabled scoring has a searcher");
            let stats = Statistics {
                data: &self.data,
                body: self.body,
                title: self.title,
            };
            self.query
                .weight(EnableScoring::enabled_from_statistics_provider(
                    &stats, searcher,
                ))
        } else {
            self.query.weight(enabled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RetrievalIndex;

    fn doc(
        id: &str,
        text: &str,
        title: &str,
        group: u64,
        sequence: i64,
        deleted: bool,
    ) -> IndexDocument {
        IndexDocument {
            id: id.into(),
            group_row: group,
            namespace_id: "snapshot".into(),
            subject_id: "snapshot".into(),
            commit_seq: sequence,
            text: text.into(),
            title: title.into(),
            deleted,
        }
    }

    #[test]
    fn live_bm25_matches_clean_rebuild_after_replacement_and_deletion() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let original = vec![
            doc(
                "a",
                "red red green blue 书写测试，中文检索",
                "red",
                0,
                1,
                false,
            ),
            doc("b", "green green red", "green", 1, 1, false),
            doc("c", "blue green green", "blue", 2, 1, false),
            doc("d", "red yellow", "yellow", 3, 1, false),
        ];
        let old_path = root.path().join("updated");
        let index = RetrievalIndex::open_for_update(&old_path)?;
        index.rebuild(&original, 1)?;
        let mut stats = LiveStats::default();
        for doc in &original {
            stats.insert(doc);
        }
        index.close()?;
        drop(index);
        stats.save(&old_path)?;
        let index = RetrievalIndex::open_for_update(&old_path)?;
        let old_hits = index.search("red green", "snapshot", "snapshot", 10)?.hits;
        let changes = vec![
            doc(
                "a",
                "green green green yellow 中文检索",
                "green",
                0,
                2,
                false,
            ),
            doc("b", "", "", 1, 2, true),
            doc("c", "blue green green", "blue", 1, 2, false),
            doc("e", "red green yellow yellow", "red", 3, 2, false),
        ];
        index.apply(&changes, 2)?;
        for change in &changes {
            if let Some(old) = original.iter().find(|old| old.id == change.id) {
                stats.remove(old)?;
            }
            if !change.deleted {
                stats.insert(change);
            }
        }
        index.close()?;
        drop(index);
        stats.save(&old_path)?;
        let updated = RetrievalIndex::open(&old_path)?;
        let expected = vec![
            changes[0].clone(),
            changes[2].clone(),
            original[3].clone(),
            changes[3].clone(),
        ];
        let full_path = root.path().join("full");
        let full = RetrievalIndex::open_for_update(&full_path)?;
        full.rebuild(&expected, 2)?;
        for query in ["red", "green", "blue yellow", "red green", "中文检索"] {
            let actual = updated.search(query, "snapshot", "snapshot", 10)?.hits;
            let baseline = full.search(query, "snapshot", "snapshot", 10)?.hits;
            assert_eq!(
                actual.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
                baseline.iter().map(|hit| &hit.id).collect::<Vec<_>>(),
                "{query}"
            );
            for (a, b) in actual.iter().zip(&baseline) {
                assert!(
                    (a.score - b.score).abs() < 1e-5,
                    "{query}: {} != {}",
                    a.score,
                    b.score
                );
            }
        }
        assert!(old_hits.iter().any(|hit| hit.id == "b"));
        assert!(
            !updated
                .search("red green", "snapshot", "snapshot", 10)?
                .hits
                .iter()
                .any(|hit| hit.id == "b")
        );
        updated.close()?;
        full.close()?;
        Ok(())
    }

    #[test]
    fn mass_deletion_compacts_postings() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let index = RetrievalIndex::open_for_update(root.path())?;
        let documents: Vec<_> = (0..520)
            .map(|i| doc(&format!("{i}"), "shared test", "title", i, 1, false))
            .collect();
        index.rebuild(&documents, 1)?;
        let removed: Vec<_> = documents
            .iter()
            .skip(1)
            .map(|item| {
                let mut value = item.clone();
                value.commit_seq = 2;
                value.deleted = true;
                value
            })
            .collect();
        index.apply(&removed, 2)?;
        assert!(index.compact_if_needed()?);
        assert!(!index.compact_if_needed()?);
        assert_eq!(
            index
                .search("shared", "snapshot", "snapshot", 10)?
                .hits
                .len(),
            1
        );
        index.close()?;
        Ok(())
    }
}
