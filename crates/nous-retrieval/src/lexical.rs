//! Rebuildable lexical projections. SQLite remains the authority for every returned hit.
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tantivy::{
    Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term,
    collector::TopDocs,
    query::{BooleanQuery, ConstScoreQuery, Occur, Query, TermQuery, TermSetQuery},
    schema::{
        Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
    },
    tokenizer::{LowerCaser, RemoveLongFilter, TextAnalyzer, TokenStream},
};
use tantivy_jieba::JiebaTokenizer;
mod scoped;

const TOKENIZER: &str = "nous_jieba";
const WRITER_BYTES: usize = 20_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDocument {
    pub id: String,
    pub namespace_id: String,
    pub subject_id: String,
    pub revision: i64,
    pub commit_seq: i64,
    pub text: String,
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
    pub revision: i64,
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
    revision: Field,
    body: Field,
}
struct Inner {
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    fields: Fields,
    coverage: RwLock<IndexCoverage>,
    scopes: RwLock<BTreeMap<String, String>>,
}

impl RetrievalIndex {
    /// Open the committed projection and its coverage from the same Tantivy metadata commit.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(path.as_ref())?;
        let expected = schema();
        let index = if path.as_ref().join("meta.json").exists() {
            Index::open_in_dir(path.as_ref()).context("cannot open lexical index")?
        } else {
            Index::create_in_dir(path.as_ref(), expected.clone())
                .context("cannot create lexical index")?
        };
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
            revision: schema.get_field("revision")?,
            body: schema.get_field("body")?,
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
        let coverage = IndexCoverage {
            through_commit_seq: payload.through_commit_seq,
            generation: payload.generation,
            document_count: reader.searcher().num_docs(),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                reader,
                writer: Mutex::new(Some(writer)),
                fields,
                coverage: RwLock::new(coverage),
                scopes: RwLock::new(payload.scopes),
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
                document.revision > 0
                    && document.commit_seq > 0
                    && document.commit_seq <= through_commit_seq,
                "index document has an invalid revision or commit sequence"
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
                    indexed.add_text(self.inner.fields.namespace, &document.namespace_id);
                    indexed.add_text(self.inner.fields.subject, &document.subject_id);
                    indexed.add_i64(self.inner.fields.revision, document.revision);
                    indexed.add_text(self.inner.fields.body, &document.text);
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

    /// Natural language is literal text; it is never parsed as Tantivy query syntax.
    pub fn search(
        &self,
        query: &str,
        namespace_id: &str,
        subject_id: &str,
        limit: usize,
    ) -> Result<SearchPage> {
        self.search_inner(query, namespace_id, subject_id, limit, None)
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
        self.search_inner(query, namespace_id, subject_id, limit, Some(allowed))
    }

    fn search_inner(
        &self,
        query: &str,
        namespace_id: &str,
        subject_id: &str,
        limit: usize,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<SearchPage> {
        let coverage = self.coverage()?;
        let terms = tokenize(query);
        if terms.is_empty() || limit == 0 || allowed.is_some_and(BTreeSet::is_empty) {
            return Ok(SearchPage {
                hits: Vec::new(),
                coverage,
            });
        }
        ensure!(limit <= 10_000, "lexical candidate limit exceeds 10000");
        let text = BooleanQuery::new(
            terms
                .into_iter()
                .map(|term| {
                    (
                        Occur::Should,
                        Box::new(TermQuery::new(
                            Term::from_field_text(self.inner.fields.body, &term),
                            IndexRecordOption::WithFreqs,
                        )) as Box<dyn Query>,
                    )
                })
                .collect(),
        );
        let exact_scope = |field, value: &str| {
            Box::new(ConstScoreQuery::new(
                Box::new(TermQuery::new(
                    Term::from_field_text(field, value),
                    IndexRecordOption::Basic,
                )),
                0.0,
            )) as Box<dyn Query>
        };
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![
            (Occur::Must, Box::new(text)),
            (
                Occur::Must,
                exact_scope(self.inner.fields.namespace, namespace_id),
            ),
            (
                Occur::Must,
                exact_scope(self.inner.fields.subject, subject_id),
            ),
        ];
        if let Some(allowed) = allowed {
            clauses.push((
                Occur::Must,
                Box::new(ConstScoreQuery::new(
                    Box::new(TermSetQuery::new(
                        allowed
                            .iter()
                            .map(|id| Term::from_field_text(self.inner.fields.id, id)),
                    )),
                    0.0,
                )),
            ));
        }
        let query = BooleanQuery::new(clauses);
        let searcher = self.inner.reader.searcher();
        let hits = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())?
            .into_iter()
            .map(|(score, address)| -> Result<SearchHit> {
                let document: TantivyDocument = searcher.doc(address)?;
                let id = document
                    .get_first(self.inner.fields.id)
                    .and_then(|v| v.as_str())
                    .context("lexical document has no identity")?
                    .to_owned();
                let revision = document
                    .get_first(self.inner.fields.revision)
                    .and_then(|v| v.as_i64())
                    .context("lexical document has no revision")?;
                Ok(SearchHit {
                    id,
                    revision,
                    score,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(SearchPage { hits, coverage })
    }
}

/// Token overlap for a bounded authority delta, using the exact same Chinese/Latin analyzer.
/// Index BM25 and this local score share matching semantics; the app owns rank fusion.
pub fn match_score(query: &str, text: &str) -> Option<f32> {
    let query = tokenize(query);
    if query.is_empty() {
        return None;
    }
    let document = tokenize(text);
    let matched = query.intersection(&document).count();
    (matched > 0).then_some(matched as f32 / query.len() as f32)
}

fn analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(JiebaTokenizer::new())
        .filter(RemoveLongFilter::limit(80))
        .filter(LowerCaser)
        .build()
}

fn tokenize(text: &str) -> HashSet<String> {
    let mut analyzer = analyzer();
    let mut stream = analyzer.token_stream(text);
    let mut terms = HashSet::new();
    while stream.advance() {
        let text = &stream.token().text;
        if text.chars().any(char::is_alphanumeric) {
            terms.insert(text.clone());
        }
    }
    terms
}

fn schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("id", STRING | STORED);
    builder.add_text_field("namespace", STRING);
    builder.add_text_field("subject", STRING);
    builder.add_i64_field("revision", STORED);
    builder.add_text_field(
        "body",
        TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(TOKENIZER)
                .set_index_option(IndexRecordOption::WithFreqs),
        ),
    );
    builder.build()
}
