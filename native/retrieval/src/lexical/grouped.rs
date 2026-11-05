//! Tantivy scores all matches; only the best passages for each group survive.
use crate::CancellationToken;
use std::collections::BTreeMap;
use tantivy::{
    DocAddress, DocId, Score, SegmentOrdinal, SegmentReader,
    collector::{Collector, SegmentCollector},
    fastfield::Column,
};
#[derive(Clone)]
pub(super) struct GroupedCollector {
    pub limit: usize,
    pub passages: usize,
    pub cancel: CancellationToken,
}
pub(super) struct GroupedSegment {
    ordinal: SegmentOrdinal,
    groups: Column<u64>,
    passages: usize,
    hits: BTreeMap<u64, Vec<(Score, DocAddress)>>,
    examined: usize,
    cancel: CancellationToken,
}
pub(super) type Fruit = (BTreeMap<u64, Vec<(Score, DocAddress)>>, usize);
fn best(hits: &mut Vec<(Score, DocAddress)>, passages: usize) {
    hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.truncate(passages);
}
impl Collector for GroupedCollector {
    type Fruit = Fruit;
    type Child = GroupedSegment;
    fn for_segment(
        &self,
        ordinal: SegmentOrdinal,
        reader: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(GroupedSegment {
            ordinal,
            groups: reader.fast_fields().u64("group")?,
            passages: self.passages,
            hits: BTreeMap::new(),
            examined: 0,
            cancel: self.cancel.clone(),
        })
    }
    fn requires_scoring(&self) -> bool {
        true
    }
    fn merge_fruits(&self, fruits: Vec<Fruit>) -> tantivy::Result<Fruit> {
        let mut groups = BTreeMap::<u64, Vec<(Score, DocAddress)>>::new();
        let mut examined = 0;
        for (fruit, count) in fruits {
            examined += count;
            for (group, hits) in fruit {
                let all = groups.entry(group).or_default();
                all.extend(hits);
                best(all, self.passages);
            }
        }
        // Keep all group maxima until merging: segment access order never chooses groups.
        Ok((groups, examined))
    }
}
impl SegmentCollector for GroupedSegment {
    type Fruit = Fruit;
    fn collect(&mut self, doc: DocId, score: Score) {
        if self.cancel.is_cancelled() {
            return;
        }
        self.examined += 1;
        let group = self.groups.first(doc).unwrap_or(0);
        let hits = self.hits.entry(group).or_default();
        hits.push((score, DocAddress::new(self.ordinal, doc)));
        best(hits, self.passages);
    }
    fn harvest(self) -> Fruit {
        (self.hits, self.examined)
    }
}
