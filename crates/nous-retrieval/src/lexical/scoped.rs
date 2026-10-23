//! Content-addressed scoped projection updates, published with Tantivy metadata.
use super::*;
use sha2::{Digest, Sha256};

impl RetrievalIndex {
    /// Synchronize one complete authority view; identical content reuses the index.
    pub fn sync_scope(
        &self,
        namespace: &str,
        subject: &str,
        documents: &[IndexDocument],
        watermark: i64,
    ) -> Result<IndexCoverage> {
        ensure!(
            !namespace.is_empty() && !subject.is_empty() && watermark >= 0,
            "lexical scope or watermark is invalid"
        );
        let mut documents = documents.to_vec();
        documents.sort_by(|a, b| a.id.cmp(&b.id));
        let mut ids = BTreeSet::new();
        for document in &documents {
            ensure!(
                document.namespace_id == namespace
                    && document.subject_id == subject
                    && ids.insert(&document.id),
                "scoped projection contains a duplicate or foreign identity"
            );
            ensure!(
                document.revision > 0
                    && document.commit_seq > 0
                    && document.commit_seq <= watermark,
                "scoped projection exceeds its authority view"
            );
        }
        let scope = serde_json::to_string(&(namespace, subject))?;
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&documents)?));
        let mut guard = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("lexical writer lock poisoned"))?;
        let writer = guard.as_mut().context("lexical index is closed")?;
        let mut scopes = self
            .inner
            .scopes
            .read()
            .map_err(|_| anyhow::anyhow!("lexical scope lock poisoned"))?
            .clone();
        if scopes.get(&scope) == Some(&digest) {
            return Ok(IndexCoverage {
                through_commit_seq: watermark,
                generation: digest,
                document_count: documents.iter().filter(|doc| !doc.deleted).count() as u64,
            });
        }
        scopes.insert(scope, digest.clone());
        let current = self.coverage()?;
        let payload = CommitPayload {
            through_commit_seq: current.through_commit_seq.max(watermark),
            generation: format!("{:x}", Sha256::digest(serde_json::to_vec(&scopes)?)),
            scopes: scopes.clone(),
        };
        let result: Result<()> = (|| {
            let term = |field, text: &str| {
                Box::new(TermQuery::new(
                    Term::from_field_text(field, text),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>
            };
            writer.delete_query(Box::new(BooleanQuery::new(vec![
                (Occur::Must, term(self.inner.fields.namespace, namespace)),
                (Occur::Must, term(self.inner.fields.subject, subject)),
            ])))?;
            for document in &documents {
                if document.deleted {
                    continue;
                }
                let mut row = TantivyDocument::default();
                row.add_text(self.inner.fields.id, &document.id);
                row.add_text(self.inner.fields.namespace, namespace);
                row.add_text(self.inner.fields.subject, subject);
                row.add_i64(self.inner.fields.revision, document.revision);
                row.add_text(self.inner.fields.body, &document.text);
                writer.add_document(row)?;
            }
            let mut commit = writer.prepare_commit()?;
            commit.set_payload(&serde_json::to_string(&payload)?);
            commit.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            writer.rollback()?;
            return Err(error.context("scoped lexical publication rolled back"));
        }
        self.inner.reader.reload()?;
        *self
            .inner
            .scopes
            .write()
            .map_err(|_| anyhow::anyhow!("lexical scope lock poisoned"))? = scopes;
        *self
            .inner
            .coverage
            .write()
            .map_err(|_| anyhow::anyhow!("lexical coverage lock poisoned"))? = IndexCoverage {
            through_commit_seq: payload.through_commit_seq,
            generation: payload.generation,
            document_count: self.inner.reader.searcher().num_docs(),
        };
        Ok(IndexCoverage {
            through_commit_seq: watermark,
            generation: digest,
            document_count: documents.iter().filter(|doc| !doc.deleted).count() as u64,
        })
    }
}
