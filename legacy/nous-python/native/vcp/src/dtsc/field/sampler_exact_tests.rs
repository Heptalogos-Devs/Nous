use super::*;
fn baseline_reference_cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut cross = 0.0;
    let mut aa = 0.0;
    let mut bb = 0.0;
    for (a, b) in a.iter().zip(b) {
        let a = f64::from(*a);
        let b = f64::from(*b);
        cross += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa > 1e-15 && bb > 1e-15 {
        cross / (aa.sqrt() * bb.sqrt())
    } else {
        0.0
    }
}
fn baseline_cosine(a: &[f32], b: &[f32]) -> f64 {
    let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
    if norm(a) > 1e-12 && norm(b) > 1e-12 {
        baseline_reference_cosine(a, b)
    } else {
        0.0
    }
}
fn baseline_sample(
    id: i64,
    vector: &[f32],
    field: &ReferenceDtscField,
    inbound: &[(i64, f64)],
    config: &ReferenceDtscFieldConfig,
) -> ReferenceDtscSample {
    let exact = field.exact_field.iter().find(|s| s.id == id);
    let maximum = inbound.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    let threshold = config.field_similarity_threshold.clamp(-1.0, 1.0);
    let mut neighbors = field
        .field_nodes
        .iter()
        .filter(|n| n.id != id)
        .filter_map(|n| {
            let similarity = baseline_cosine(vector, &n.vector);
            if similarity < threshold {
                return None;
            }
            let local = unit((similarity - threshold) / (1.0 - threshold).max(1e-6));
            let specific = if maximum <= 0.0 {
                1.0
            } else {
                config.public_hub_floor.clamp(0.05, 1.0).max(
                    1.0 - unit(
                        inbound
                            .iter()
                            .find(|(id, _)| *id == n.id)
                            .map_or(0.0, |(_, w)| *w)
                            / maximum,
                    )
                    .sqrt(),
                )
            };
            Some((
                n.potential * local.powf(config.field_kernel_exponent.max(0.25)) * specific,
                n,
            ))
        })
        .collect::<Vec<_>>();
    neighbors.sort_by(|a, b| b.0.total_cmp(&a.0));
    neighbors.truncate(config.max_field_neighbors.max(1));
    let interpolated = if neighbors.is_empty() {
        0.0
    } else {
        neighbors.iter().map(|(p, _)| *p).sum::<f64>() / (neighbors.len() as f64).sqrt()
    };
    ReferenceDtscSample {
        id,
        potential: unit(exact.map_or(0.0, |s| s.potential).max(interpolated)),
        exact: exact.is_some(),
        source_type: exact
            .map(|s| s.source_type.clone())
            .or_else(|| neighbors.first().map(|(_, n)| n.source_type.clone()))
            .unwrap_or_else(|| "unknown".into()),
    }
}

fn vectors() -> Vec<Vec<f32>> {
    let mut result = vec![
        vec![],
        vec![0.0],
        vec![-0.0],
        vec![1e-6],
        vec![1e-7],
        vec![f32::INFINITY],
        vec![f32::NEG_INFINITY],
        vec![f32::NAN],
        vec![f32::from_bits(0x7fa12345)],
        vec![f32::MAX],
        vec![1.0, f32::INFINITY],
        vec![1.0, f32::NAN],
        vec![1.0, 0.0],
        vec![0.0, 1.0],
        vec![-1.0, 0.0],
        vec![1e-6, 1e-6],
    ];
    let mut seed = 42_u64;
    for dim in [1, 2, 3, 7, 16, 31, 128, 512] {
        for _ in 0..8 {
            result.push(
                (0..dim)
                    .map(|_| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        (seed >> 32) as u32 as f32 / u32::MAX as f32 - 0.5
                    })
                    .collect(),
            );
        }
    }
    result
}
#[test]
fn cosine_matches_reference_bits() {
    let vectors = vectors();
    for (i, a) in vectors.iter().enumerate() {
        for (j, b) in vectors.iter().enumerate() {
            assert_eq!(
                dtsc_cosine(a, b).to_bits(),
                baseline_cosine(a, b).to_bits(),
                "vector pair {i}, {j}"
            );
        }
    }
}
#[test]
fn sampler_matches_reference_bits_and_order() {
    let vectors = vectors();
    let field = ReferenceDtscField {
        exact_field: vec![
            ReferenceDtscSample {
                id: 3,
                potential: 0.4,
                exact: true,
                source_type: "first".into(),
            },
            ReferenceDtscSample {
                id: 3,
                potential: 0.9,
                exact: true,
                source_type: "duplicate".into(),
            },
        ],
        field_nodes: vectors
            .iter()
            .enumerate()
            .map(|(i, vector)| ReferenceDtscFieldNode {
                id: (i % 13) as i64,
                potential: (i % 4) as f64 / 3.0,
                vector: vector.clone(),
                source_type: format!("node-{i}"),
            })
            .collect(),
        field_entropy: 0.0,
        field_trusted: true,
        total_energy: 0.0,
        max_energy: 0.0,
        samples: vec![],
    };
    for inbound in [
        vec![],
        vec![(1, 1.0), (1, 0.2), (3, 0.5)],
        vec![(1, f64::NAN), (3, f64::INFINITY)],
        vec![(1, -1.0)],
    ] {
        for threshold in [-1.0, 0.0, 0.5, 1.0, f64::NAN] {
            for neighbors in [0, 1, 4, 128] {
                let config = ReferenceDtscFieldConfig {
                    field_similarity_threshold: threshold,
                    max_field_neighbors: neighbors,
                    ..Default::default()
                };
                let sampler = DtscFieldSampler::new(&field, &inbound, &config);
                for (i, vector) in vectors.iter().enumerate() {
                    let id = (i % 17) as i64;
                    let actual = sampler.sample(id, vector);
                    let expected = baseline_sample(id, vector, &field, &inbound, &config);
                    assert_eq!(
                        actual.potential.to_bits(),
                        expected.potential.to_bits(),
                        "vector {i}, threshold {threshold}, neighbors {neighbors}"
                    );
                    assert_eq!(actual.id, expected.id);
                    assert_eq!(actual.exact, expected.exact);
                    assert_eq!(actual.source_type, expected.source_type);
                }
            }
        }
    }
}
