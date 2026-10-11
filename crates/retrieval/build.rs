// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, fs, path::Path};

fn sources(root: &Path, path: &Path, files: &mut BTreeMap<String, std::path::PathBuf>) {
    if path.is_dir() {
        println!("cargo:rerun-if-changed={}", path.display());
        for entry in fs::read_dir(path).expect("projection sources") {
            sources(root, &entry.expect("projection source entry").path(), files);
        }
    } else {
        let name = path
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/");
        files.insert(name, path.to_owned());
    }
}

fn main() {
    // Projection builders and their input owners determine asset compatibility.
    // Catalog presentation and runtime query policy are deliberately separate.
    let root = Path::new("../..");
    let mut files = BTreeMap::new();
    for path in [
        "Cargo.lock",
        "crates/retrieval/build.rs",
        "crates/retrieval/src/assets/files.rs",
        "crates/retrieval/src/assets/build.rs",
        "crates/retrieval/src/concept/generation.rs",
        "crates/retrieval/src/mechanisms/dense.rs",
        "crates/retrieval/src/assets/family.rs",
        "crates/retrieval/src/mechanisms/graph.rs",
        "crates/retrieval/src/mechanisms/lexical.rs",
        "crates/retrieval/src/material.rs",
        "crates/retrieval/src/embedding.rs",
        "crates/retrieval/src/projection.rs",
        "crates/material/src/service/materialization.rs",
        "crates/material/src/service/documents.rs",
        "crates/material/src/service/lineage.rs",
        "crates/memory/src/service/provenance.rs",
        "crates/memory/src/service/schema/read.rs",
        "crates/persistence/src/projection/lookup.rs",
        "crates/persistence/src/historical/text.rs",
        "crates/persistence/src/episode_text.rs",
        "crates/retrieval/src/provider.rs",
        "crates/retrieval/src/mechanisms/residual.rs",
        "crates/retrieval/src/vcp/adapter.rs",
        "crates/retrieval/src/vcp/generation.rs",
        "crates/retrieval/src/vcp/graph.rs",
        "crates/retrieval/src/vcp/index.rs",
        "crates/retrieval/src/vcp/material.rs",
        "crates/retrieval/src/reference",
        "crates/core/src/concept.rs",
        "crates/persistence/src/projection/text.rs",
        "crates/core/src/history.rs",
        "crates/core/src/cognition.rs",
        "crates/persistence/src/historical/projection.rs",
        "crates/persistence/src/projection/topology.rs",
    ] {
        sources(root, &root.join(path), &mut files);
    }
    let mut hash = blake3::Hasher::new();
    for (name, path) in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let content = fs::read_to_string(&path)
            .expect("projection source UTF-8")
            .replace("\r\n", "\n");
        hash.update(&(name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update(&(content.len() as u64).to_le_bytes());
        hash.update(content.as_bytes());
    }
    println!(
        "cargo:rustc-env=NOUS_SERVING_IMPLEMENTATION_DIGEST={}",
        hash.finalize().to_hex()
    );
}
