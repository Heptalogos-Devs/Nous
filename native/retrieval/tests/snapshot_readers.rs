//! Frozen snapshots can be opened and queried concurrently without writer locks.
use anyhow::Result;
use nous_retrieval::{
    CancellationToken, RetrievalIndex,
    bundles::{Associations, Candidates, Concepts, Matrix, SourceFacts},
    query::Options,
    snapshot::{Input, Snapshot, build},
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Barrier},
};

#[test]
fn simultaneous_snapshot_readers_do_not_acquire_writer_lock() -> Result<()> {
    let temporary_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp");
    std::fs::create_dir_all(&temporary_root)?;
    let directory = tempfile::tempdir_in(temporary_root)?;
    let component = directory.path().join("component");
    let identity = json!({"snapshot_id":"readers", "space_signature":"none", "build_config_signature":"build"});
    let input = Input {
        candidates: Candidates {
            metadata_json: json!({"keys":["body","other","second"],"documents":["doc","doc","doc2"],"titles":["","",""],
                "group_keys":["doc","doc2"],"group_rows":[0,0,1],"local":[false,true,false]})
            .to_string(),
            texts: Arc::new(vec!["A needle stays available to both readers.".into(),
                "Another needle in the first document.".into(),"A needle in the second document.".into()]),
            vectors: Some(Matrix {
                values: Arc::new(vec![1.0, 0.0, 0.9, 0.1, 0.8, 0.2]),
                rows: 3,
                cols: 2,
            }),
            membership_indptr: Arc::new(vec![0, 0, 0, 0]),
            membership_indices: Arc::new(vec![]),
            path_indptr: Arc::new(vec![0, 0, 0, 0]),
            path_step_indptr: Arc::new(vec![0]),
            path_indices: Arc::new(vec![]),
            path_positions: Arc::new(vec![]),
            root_indptr: Arc::new(vec![0, 0, 0, 0]),
            root_indices: Arc::new(vec![]),
        },
        concepts: Concepts {
            metadata_json: json!({"keys":[],"labels":[],"aliases":[]}).to_string(),
            vectors: None,
        },
        facts: SourceFacts {
            metadata_json: json!({"root_keys":[],"root_checksums":[],"sequence_keys":[]})
                .to_string(),
            sequence_roots: Arc::new(vec![]),
            indptr: Arc::new(vec![0]),
            indices: Arc::new(vec![]),
            positions: Arc::new(vec![]),
            ordered: Arc::new(vec![]),
        },
        associations: Associations {
            metadata_json: json!({"support_groups":[],"keys":[],"relations":[]}).to_string(),
            sources: Arc::new(vec![]),
            targets: Arc::new(vec![]),
            directed: Arc::new(vec![]),
            strengths: Arc::new(vec![]),
            confidences: Arc::new(vec![]),
            polarities: Arc::new(vec![]),
            edge_groups: Arc::new(vec![]),
            group_root_indptr: Arc::new(vec![0]),
            group_root_indices: Arc::new(vec![]),
        },
        identity: identity.clone(),
        options: json!({}),
    };
    let token = CancellationToken::default();
    build(input, &component, None, &token)?;
    // Holding the staging writer makes any accidental writer acquisition by
    // either frozen reader fail immediately, while the published data stays fixed.
    let writer = RetrievalIndex::open_for_update(component.join("lexical"))?;
    let barrier = Barrier::new(2);
    let program = json!({
        "nodes":[
            {"id":"text","operator":"lexical.search","inputs":{"text":{"node":"query","port":"lexical"},
                "snapshot":{"node":"snapshot","port":"handle"},"allowed":{"node":"scope","port":"allowed"}},"parameters":{}},
            {"id":"rank","operator":"rank.from_lane","inputs":{"candidates":{"node":"text","port":"candidates"}},"parameters":{"lane":"bm25"}},
            {"id":"locate","operator":"passages.locate","inputs":{"ranked":{"node":"rank","port":"ranked"},
                "intent":{"node":"query","port":"intent"},"snapshot":{"node":"snapshot","port":"handle"}},"parameters":{"signals":["lexical"]}}
        ],
        "outputs":{"evidence":{"node":"locate","port":"passages"}}
    }).to_string();
    std::thread::scope(|scope| -> Result<()> {
        let handles = (0..2)
            .map(|_| {
                let barrier = &barrier;
                let component = &component;
                let identity = &identity;
                let program = &program;
                let token = &token;
                scope.spawn(move || -> Result<()> {
                    barrier.wait();
                    let snapshot = Snapshot::open(component, identity, token)?;
                    let result = snapshot.execute_program(
                        "readers",
                        program,
                        "needle",
                        None,
                        None,
                        &BTreeSet::from([0, 1, 2]),
                        Options::default(),
                        vec![],
                        None,
                        token,
                    )?;
                    assert_eq!(result.rows.len(), 3);
                    Ok(())
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("reader thread panicked"))??;
        }
        Ok(())
    })?;
    let snapshot = Snapshot::open(&component, &identity, &token)?;
    let mut rankings = Vec::new();
    for passages_per_group in [1, 2] {
        let options = Options {
            grouping: "document".into(),
            passages_per_group,
            candidates_per_group: 2,
            include_trace: true,
            ..Options::default()
        };
        let result = snapshot.execute_program(
            "quota",
            &program,
            "needle",
            None,
            None,
            &BTreeSet::from([0, 1, 2]),
            options,
            vec![],
            None,
            &token,
        )?;
        let raw: serde_json::Value = serde_json::from_str(&result.result_json)?;
        assert_eq!(raw["coverage"]["ranked_candidates"], 3);
        rankings.push(raw["diagnostics"]["ranked_nodes"]["rank.ranked"]["hits"].clone());
    }
    assert_eq!(rankings[0], rankings[1]);

    let dense_program = json!({
        "nodes":[
            {"id":"dense","operator":"ann.search","inputs":{"vector":{"node":"query","port":"raw_vector"},
                "snapshot":{"node":"snapshot","port":"handle"},"allowed":{"node":"scope","port":"allowed"}},"parameters":{}},
            {"id":"rank","operator":"rank.from_lane","inputs":{"candidates":{"node":"dense","port":"candidates"}},"parameters":{"lane":"query_knn"}},
            {"id":"locate","operator":"passages.locate","inputs":{"ranked":{"node":"rank","port":"ranked"},
                "intent":{"node":"query","port":"intent"},"snapshot":{"node":"snapshot","port":"handle"}},"parameters":{"signals":["lexical"]}}
        ],
        "outputs":{"evidence":{"node":"locate","port":"passages"}}
    }).to_string();
    let mut dense_rankings = Vec::new();
    for passages_per_group in [1, 2] {
        let options = Options {
            query_space_signature: Some("none".into()),
            grouping: "document".into(),
            passages_per_group,
            candidates_per_group: 2,
            include_trace: true,
            ..Options::default()
        };
        let result = snapshot.execute_program(
            "dense-quota",
            &dense_program,
            "needle",
            Some(&[1.0, 0.0]),
            None,
            &BTreeSet::from([0, 1, 2]),
            options,
            vec![],
            None,
            &token,
        )?;
        let raw: serde_json::Value = serde_json::from_str(&result.result_json)?;
        assert_eq!(raw["coverage"]["ranked_candidates"], 3);
        dense_rankings.push(raw["diagnostics"]["ranked_nodes"]["rank.ranked"]["hits"].clone());
    }
    assert_eq!(dense_rankings[0], dense_rankings[1]);
    writer.close()?;
    Ok(())
}
