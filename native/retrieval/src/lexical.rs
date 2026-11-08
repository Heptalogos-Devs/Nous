//! Immutable lexical indexes over caller-owned frozen candidate rows.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tantivy::{
    Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term,
    collector::TopDocs,
    query::{BooleanQuery, ConstScoreQuery, Occur, Query, TermQuery, TermSetQuery},
    schema::{Field, IndexRecordOption, Value},
    snippet::SnippetGenerator,
};
mod grouped;
pub(crate) mod live;
mod scoped;
mod search;
mod text;
pub use text::match_score;
use text::{analyzer, schema, tokenize};
#[cfg(windows)]
mod windows_directory;

const TOKENIZER: &str = "nous_jieba";
const WRITER_BYTES: usize = 20_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDocument {
    pub id: String,
    #[serde(default)]
    pub group_row: u64,
    pub namespace_id: String,
    pub subject_id: String,
    pub commit_seq: i64,
    pub text: String,
    pub title: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexCoverage {
    pub through_commit_seq: i64,
    pub generation: String,
    pub document_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub coverage: IndexCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommitPayload {
    through_commit_seq: i64,
    generation: String,
    scopes: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct RetrievalIndex {
    inner: Arc<Inner>,
}

struct Fields {
    id: Field,
    namespace: Field,
    subject: Field,
    body: Field,
    title: Field,
    group: Field,
}
struct Inner {
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
    coverage: RwLock<IndexCoverage>,
    scopes: RwLock<BTreeMap<String, String>>,
    live_stats: Option<Arc<live::LiveStats>>,
}

impl RetrievalIndex {
    /// Open the committed projection and its coverage from the same Tantivy metadata commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(path.as_ref())?;
        let expected = schema();
        #[cfg(windows)]
        let directory = windows_directory::WindowsDirectory::open(path.as_ref())?;
        #[cfg(not(windows))]
        let directory = tantivy::directory::MmapDirectory::open(path.as_ref())?;
        let index = Index::open_or_create(directory, expected.clone())
            .context("cannot open or create lexical index")?;
        ensure!(
            index.schema() == expected,
            "lexical schema changed; rebuild the projection"
        );
        index.tokenizers().register(TOKENIZER, analyzer());
        let schema = index.schema();
        let fields = Fields {
            id: schema.get_field("id")?,
            namespace: schema.get_field("namespace")?,
            subject: schema.get_field("subject")?,
            body: schema.get_field("body")?,
            title: schema.get_field("title")?,
            group: schema.get_field("group")?,
        };
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        let mut writer: IndexWriter = index.writer_with_num_threads(1, WRITER_BYTES)?;
        let payload: CommitPayload = match index.load_metas()?.payload {
            Some(payload) => {
                serde_json::from_str(&payload).context("invalid lexical coverage metadata")?
            }
            None => {
                let payload = CommitPayload {
                    through_commit_seq: 0,
                    generation: uuid::Uuid::new_v4().to_string(),
                    scopes: BTreeMap::new(),
                };
                let mut commit = writer.prepare_commit()?;
                commit.set_payload(&serde_json::to_string(&payload)?);
                commit.commit()?;
                reader.reload()?;
                payload
            }
        };
        let live_stats = live::LiveStats::load(path.as_ref())?.map(Arc::new);
        if let Some(stats) = &live_stats {
            ensure!(
                stats.count() as u64 == reader.searcher().num_docs(),
                "live lexical statistics disagree with index"
            );
        }
        let coverage = IndexCoverage {
            through_commit_seq: payload.through_commit_seq,
            generation: payload.generation,
            document_count: reader.searcher().num_docs(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                index,
                reader,
                writer: Mutex::new(Some(writer)),
                fields,
                coverage: RwLock::new(coverage),
                scopes: RwLock::new(payload.scopes),
                live_stats,
            }),
        })
    }

    pub fn coverage(&self) -> Result<IndexCoverage> {
        Ok(self
            .inner
            .coverage
            .read()
            .map_err(|_| anyhow::anyhow!("lexical coverage lock poisoned"))?
            .clone())
    }

    /// Stop indexing and release the filesystem writer lock for an in-process reopen.
    pub fn close(&self) -> Result<()> {
        let mut writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical writer lock poisoned"))?;
        if let Some(writer) = writer.take() {
            writer.wait_merging_threads()?;
        }
        Ok(())
    }

    /// Apply an authority delta. Each stable ID is replaced or removed in one durable commit.
    pub fn apply(
        &self,
        changes: &[IndexDocument],
        through_commit_seq: i64,
    ) -> Result<IndexCoverage> {
        self.write(changes, through_commit_seq, false)
    }

    /// Explicit repair/first population; readers retain the old committed snapshot until publish.
    pub fn rebuild(
        &self,
        documents: &[IndexDocument],
        through_commit_seq: i64,
    ) -> Result<IndexCoverage> {
        self.write(documents, through_commit_seq, true)
    }

    fn write(
        &self,
        documents: &[IndexDocument],
        through_commit_seq: i64,
        rebuild: bool,
    ) -> Result<IndexCoverage> {
        ensure!(
            through_commit_seq >= 0,
            "index coverage must be nonnegative"
        );
        for document in documents {
            ensure!(
                !document.id.is_empty()
                    && !document.namespace_id.is_empty()
                    && !document.subject_id.is_empty(),
                "index document requires stable identity and scope"
            );
            ensure!(
                document.commit_seq > 0 && document.commit_seq <= through_commit_seq,
                "index document has an invalid commit sequence"
            );
        }
        let mut writer_guard = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical writer lock poisoned"))?;
        let writer = writer_guard.as_mut().context("lexical index is closed")?;
        let current = self.coverage()?;
        ensure!(
            through_commit_seq >= current.through_commit_seq,
            "lexical coverage cannot move backwards"
        );
        if !rebuild && through_commit_seq == current.through_commit_seq {
            return Ok(current);
        }
        let generation = if rebuild {
            uuid::Uuid::new_v4().to_string()
        } else {
            current.generation
        };
        let payload = CommitPayload {
            through_commit_seq,
            generation: generation.clone(),
            scopes: BTreeMap::new(),
        };
        let result: Result<()> = (|| {
            if rebuild {
                writer.delete_all_documents()?;
            }
            for document in documents {
                writer.delete_term(Term::from_field_text(self.inner.fields.id, &document.id));
                if !document.deleted {
                    let mut indexed = TantivyDocument::default();
                    indexed.add_text(self.inner.fields.id, &document.id);
                    indexed.add_u64(self.inner.fields.group, document.group_row);
                    indexed.add_text(self.inner.fields.namespace, &document.namespace_id);
                    indexed.add_text(self.inner.fields.subject, &document.subject_id);
                    indexed.add_text(self.inner.fields.body, &document.text);
                    indexed.add_text(self.inner.fields.title, &document.title);
                    writer.add_document(indexed)?;
                }
            }
            let mut commit = writer.prepare_commit()?;
            commit.set_payload(&serde_json::to_string(&payload)?);
            commit.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            writer
                .rollback()
                .context("lexical write failed and rollback failed")?;
            return Err(error.context("lexical write rolled back; previous reader retained"));
        }
        // Reload precedes publishing the corresponding watermark. A concurrent reader can
        // see newer documents with an older conservative watermark, never the reverse.
        self.inner
            .reader
            .reload()
            .context("lexical commit is durable but reader reload failed")?;
        let coverage = IndexCoverage {
            through_commit_seq,
            generation,
            document_count: self.inner.reader.searcher().num_docs(),
        };
        *self
            .inner
            .coverage
            .write()
            .map_err(|_| anyhow::anyhow!("lexical coverage lock poisoned"))? = coverage.clone();
        self.inner
            .scopes
            .write()
            .map_err(|_| anyhow::anyhow!("lexical scope lock poisoned"))?
            .clear();
        Ok(coverage)
    }

    /// Bound deleted postings and segment growth without mutating any earlier snapshot.
    pub(crate) fn compact_if_needed(&self) -> Result<bool> {
        let searcher = self.inner.reader.searcher();
        let segments = searcher.segment_readers();
        let total: u64 = segments
            .iter()
            .map(|segment| u64::from(segment.max_doc()))
            .sum();
        let live = searcher.num_docs();
        if total.saturating_sub(live) <= live.max(512) && segments.len() <= 32 {
            return Ok(false);
        }
        drop(searcher);
        let mut guard = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical writer lock poisoned"))?;
        let writer = guard.as_mut().context("lexical index is closed")?;
        let segments = self.inner.index.searchable_segment_ids()?;
        if !segments.is_empty() {
            writer.merge(&segments).wait()?;
            self.inner.reader.reload()?;
        }
        Ok(true)
    }

    /// Natural language is literal text; it is never parsed as Tantivy query syntax.
    pub fn search(
        &self,
        query: &str,
        namespace_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<SearchPage> {
        self.search_inner(query, namespace_id, subject_id, limit, None, None)
    }

    /// Exact admissible identities restrict the native collector before Top-K.
    pub fn search_allowed(
        &self,
        query: &str,
        namespace_id: &str,
        subject_id: &str,
        limit: usize,
        allowed: &BTreeSet<String>,
    ) -> Result<SearchPage> {
        self.search_inner(query, namespace_id, subject_id, limit, Some(allowed), None)
    }

    /// Select source offsets with the same analyzer as the lexical index.
    pub fn evidence(&self, query: &str, body: &str, title: &str) -> serde_json::Value {
        let terms = tokenize(query)
            .into_iter()
            .map(|term| (term, 1.0))
            .collect();
        let body_generator = SnippetGenerator::new(terms, analyzer(), self.inner.fields.body, 500);
        let body_snippet = body_generator.snippet(body);
        let title_terms = tokenize(query)
            .into_iter()
            .map(|term| (term, 1.0))
            .collect();
        let title_generator =
            SnippetGenerator::new(title_terms, analyzer(), self.inner.fields.title, 500);
        let title_snippet = title_generator.snippet(title);
        let mut fields = Vec::new();
        if !title_snippet.is_empty() {
            fields.push("title");
        }
        if !body_snippet.is_empty() {
            fields.push("body");
        }
        let range = body_snippet.fragment_range();
        serde_json::json!({
            "matched_fields": fields,
            "body_range": if body_snippet.is_empty() { None } else { Some([range.start, range.end]) },
        })
    }
}
