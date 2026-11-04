//! Ordered greedy deduplication without reassociating a dot product.

#[derive(Default)]
pub(crate) struct OrderedDedup {
    blocks: Vec<Vec<[f64; 4]>>,
    len: usize,
}

impl OrderedDedup {
    pub(crate) fn push(&mut self, vector: &[f32]) {
        let lane = self.len % 4;
        if lane == 0 {
            self.blocks.push(vec![[0.0; 4]; vector.len()]);
        }
        let block = self.blocks.last_mut().unwrap();
        debug_assert_eq!(block.len(), vector.len());
        for (columns, value) in block.iter_mut().zip(vector) {
            columns[lane] = f64::from(*value);
        }
        self.len += 1;
    }

    pub(crate) fn first_match(&self, vector: &[f32], threshold: f64) -> Option<(usize, f64)> {
        for (block_index, block) in self.blocks.iter().enumerate() {
            let sums = ordered_dots(vector, block);
            let occupied = (self.len - block_index * 4).min(4);
            for (lane, cosine) in sums.into_iter().take(occupied).enumerate() {
                if cosine >= threshold {
                    return Some((block_index * 4 + lane, cosine));
                }
            }
        }
        None
    }
}

fn ordered_dots(vector: &[f32], block: &[[f64; 4]]) -> [f64; 4] {
    // Each lane is one original left-to-right f64 sum, including Iterator::sum's
    // negative-zero identity. The layout enables SIMD *across representatives*,
    // never horizontal reassociation or fused multiply-add within a dot product.
    // Checking lanes in insertion order retains the first matching representative;
    // inactive lanes in the final block are never considered by first_match.
    let mut sums = [-0.0; 4];
    for (&value, columns) in vector.iter().zip(block) {
        let value = f64::from(value);
        sums[0] += value * columns[0];
        sums[1] += value * columns[1];
        sums[2] += value * columns[2];
        sums[3] += value * columns[3];
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum()
    }

    fn value(state: &mut u64) -> f32 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*state >> 32) as i32) as f32 / i32::MAX as f32
    }

    #[test]
    fn every_lane_matches_scalar_bits() {
        let mut state = 137;
        for dim in [0, 1, 2, 3, 4, 7, 31, 128, 511, 512, 1536] {
            let mut rows = (0..17)
                .map(|_| (0..dim).map(|_| value(&mut state)).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            if dim > 0 {
                rows[0].fill(-0.0);
                rows[1].fill(0.0);
                rows[2].fill(f32::from_bits(1));
                rows[3].fill(f32::MAX);
            }
            let mut dedup = OrderedDedup::default();
            for row in &rows {
                dedup.push(row);
            }
            for query in &rows {
                for (b, block) in dedup.blocks.iter().enumerate() {
                    let actual = ordered_dots(query, block);
                    for lane in 0..(rows.len() - 4 * b).min(4) {
                        assert_eq!(
                            actual[lane].to_bits(),
                            scalar(query, &rows[4 * b + lane]).to_bits(),
                            "dimension {dim}, block {b}, lane {lane}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn first_match_preserves_greedy_order_and_threshold_ties() {
        let mut state = 1337;
        for dim in [1, 3, 32, 128, 512] {
            let rows = (0..101)
                .map(|_| (0..dim).map(|_| value(&mut state)).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            for threshold in [-1.0, -0.0, 0.1, 0.95, 1.0, scalar(&rows[0], &rows[1])] {
                let mut expected = Vec::<&[f32]>::new();
                let mut actual = OrderedDedup::default();
                for row in rows.iter().chain(rows.iter().rev()) {
                    let reference = expected.iter().enumerate().find_map(|(i, other)| {
                        let score = scalar(row, other);
                        (score >= threshold).then_some((i, score.to_bits()))
                    });
                    assert_eq!(
                        actual
                            .first_match(row, threshold)
                            .map(|(i, s)| (i, s.to_bits())),
                        reference
                    );
                    if reference.is_none() {
                        expected.push(row);
                        actual.push(row);
                    }
                }
            }
        }
    }

    #[test]
    fn unoccupied_lanes_never_match() {
        let mut dedup = OrderedDedup::default();
        assert!(dedup.first_match(&[0.0], -1.0).is_none());
        dedup.push(&[-1.0]);
        assert!(dedup.first_match(&[1.0], -0.0).is_none());
        dedup.push(&[1.0]);
        dedup.push(&[1.0]);
        assert_eq!(dedup.first_match(&[1.0], 1.0), Some((1, 1.0)));
    }
}
