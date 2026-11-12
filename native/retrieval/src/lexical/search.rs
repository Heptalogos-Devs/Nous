//! Scoped lexical matching, including stable grouped ranking.
use super::*;

impl RetrievalIndex {
    pub fn search_grouped(
        &self,
        query: &str,
        allowed: &BTreeSet<String>,
        limit: usize,
        passages: usize,
        ranks: std::sync::Arc<std::collections::BTreeMap<u64, u64>>,
        cancel: &crate::CancellationToken,
    ) -> Result<(SearchPage, crate::grouping::LaneCoverage)> {
        let collector = grouped::GroupedCollector {
            limit,
            passages,
            ranks,
            cancel: cancel.clone(),
        };
        let mut coverage = crate::grouping::LaneCoverage {
            recall_exhausted: true,
            eligible_groups_known: Some(0),
            ..Default::default()
        };
        let page = self.search_inner(
            query,
            "snapshot",
            "snapshot",
            limit,
            Some(allowed),
            Some((&collector, &mut coverage)),
        )?;
        cancel.check()?;
        Ok((page, coverage))
    }

    pub(super) fn search_inner(
        &self,
        query: &str,
        namespace_id: &str,
        subject_id: &str,
        limit: usize,
        allowed: Option<&BTreeSet<String>>,
        grouped: Option<(
            &grouped::GroupedCollector,
            &mut crate::grouping::LaneCoverage,
        )>,
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
                .flat_map(|term| {
                    [self.inner.fields.body, self.inner.fields.title].map(move |field| {
                        (
                            Occur::Should,
                            Box::new(TermQuery::new(
                                Term::from_field_text(field, &term),
                                IndexRecordOption::WithFreqs,
                            )) as Box<dyn Query>,
                        )
                    })
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
        let text: Box<dyn Query> = if let Some(stats) = &self.inner.live_stats {
            Box::new(live::LiveQuery {
                query: text,
                data: stats.clone(),
                body: self.inner.fields.body,
                title: self.inner.fields.title,
            })
        } else {
            Box::new(text)
        };
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![
            (Occur::Must, text),
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
        let scored = if let Some((collector, coverage)) = grouped {
            let (groups, examined) = searcher.search(&query, collector)?;
            collector.cancel.check()?;
            coverage.examined_rows = examined;
            coverage.eligible_groups_known = Some(groups.len());
            coverage.recall_exhausted = true;
            coverage.truncated = groups.len() > collector.limit;
            coverage.reason = coverage.truncated.then(|| "lane_group_limit".into());
            let mut groups = groups.into_iter().collect::<Vec<_>>();
            ensure!(
                groups
                    .iter()
                    .all(|(id, _)| collector.ranks.contains_key(id)),
                "unknown lexical group identity"
            );
            groups.sort_by(|a, b| {
                b.1[0]
                    .0
                    .total_cmp(&a.1[0].0)
                    .then(collector.ranks[&a.0].cmp(&collector.ranks[&b.0]))
            });
            let mut hits = groups
                .into_iter()
                .take(collector.limit)
                .flat_map(|(group, hits)| {
                    let rank = collector.ranks[&group];
                    hits.into_iter()
                        .map(move |(score, address)| (score, address, rank))
                })
                .collect::<Vec<_>>();
            hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
            hits.into_iter()
                .map(|(score, address, _)| (score, address))
                .collect()
        } else {
            searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?
        };
        let hits = scored
            .into_iter()
            .map(|(score, address)| -> Result<SearchHit> {
                let document: TantivyDocument = searcher.doc(address)?;
                let id = document
                    .get_first(self.inner.fields.id)
                    .and_then(|v| v.as_str())
                    .context("lexical document has no identity")?
                    .to_owned();
                Ok(SearchHit { id, score })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(SearchPage { hits, coverage })
    }
}
