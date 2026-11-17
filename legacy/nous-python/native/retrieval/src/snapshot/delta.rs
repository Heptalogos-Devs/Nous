//! Derive a new immutable lexical component from a borrowed snapshot.
use super::{CandidateMeta, Frozen, Input, StableIds};
use crate::{CancellationToken, IndexDocument, RetrievalIndex, lexical::live::LiveStats};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
};

fn document(
    input: &Input,
    meta: &CandidateMeta,
    ids: &StableIds,
    row: usize,
    sequence: i64,
) -> IndexDocument {
    IndexDocument {
        id: meta.keys[row].clone(),
        group_row: ids.group_for(meta, row),
        namespace_id: "snapshot".into(),
        subject_id: "snapshot".into(),
        commit_seq: sequence,
        text: input.candidates.texts[row].clone(),
        title: meta.titles[row].clone(),
        deleted: false,
    }
}

fn copy_files(source: &Path, target: &Path, token: &CancellationToken) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        token.check()?;
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".lock") {
            continue;
        }
        ensure!(
            entry.file_type()?.is_file(),
            "unsupported lexical component type"
        );
        let mut from = std::fs::File::open(entry.path())?;
        let mut to = std::fs::File::create(target.join(name))?;
        let mut buffer = [0u8; 65536];
        loop {
            token.check()?;
            let n = from.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            to.write_all(&buffer[..n])?;
        }
        to.sync_all()?;
    }
    Ok(())
}

pub(super) fn compatible(input: &Input, base: &Frozen) -> bool {
    input.identity.get("space_signature") == base.input.identity.get("space_signature")
        && input.identity.get("build_config_signature")
            == base.input.identity.get("build_config_signature")
        && input.options == base.input.options
}

pub(super) fn build_lexical(
    input: &Input,
    meta: &CandidateMeta,
    ids: &StableIds,
    path: &Path,
    base: Option<&Frozen>,
    token: &CancellationToken,
) -> Result<Value> {
    let source = base.filter(|old| compatible(input, old));
    let old_stats = source
        .map(|old| LiveStats::load(&old.path.join("lexical")))
        .transpose()?
        .flatten();
    if let (Some(old), Some(mut stats)) = (source, old_stats) {
        copy_files(&old.path.join("lexical"), path, token)?;
        let index = RetrievalIndex::open(path)?;
        let sequence = index
            .coverage()?
            .through_commit_seq
            .checked_add(1)
            .context("lexical coverage overflow")?;
        let previous: HashMap<_, _> = old
            .candidates
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| (key.as_str(), i))
            .collect();
        let next: HashMap<_, _> = meta
            .keys
            .iter()
            .enumerate()
            .map(|(i, key)| (key.as_str(), i))
            .collect();
        let mut changes = Vec::new();
        for (key, &row) in &previous {
            token.check()?;
            if !next.contains_key(key) {
                let mut deleted = document(&old.input, &old.candidates, &old.ids, row, sequence);
                deleted.deleted = true;
                changes.push(deleted);
            }
        }
        for (row, key) in meta.keys.iter().enumerate() {
            token.check()?;
            let changed = previous.get(key.as_str()).is_none_or(|&old_row| {
                old.candidates.group_keys[old.candidates.group_rows[old_row] as usize]
                    != meta.group_keys[meta.group_rows[row] as usize]
                    || old.candidates.titles[old_row] != meta.titles[row]
                    || old.input.candidates.texts[old_row] != input.candidates.texts[row]
            });
            if changed {
                changes.push(document(input, meta, ids, row, sequence));
            }
        }
        if !changes.is_empty() {
            index.apply(&changes, sequence)?;
            for change in &changes {
                token.check()?;
                let old_row = previous.get(change.id.as_str()).copied();
                let same_text = old_row.is_some_and(|row| {
                    old.input.candidates.texts[row] == change.text
                        && old.candidates.titles[row] == change.title
                });
                if change.deleted || !same_text {
                    if let Some(row) = old_row {
                        stats.remove(&document(
                            &old.input,
                            &old.candidates,
                            &old.ids,
                            row,
                            sequence,
                        ))?;
                    }
                    if !change.deleted {
                        stats.insert(change);
                    }
                }
            }
        }
        ensure!(
            stats.count() == meta.keys.len(),
            "updated lexical statistics disagree with candidates"
        );
        let compacted = index.compact_if_needed()?;
        index.close()?;
        drop(index);
        stats.save(path)?;
        Ok(json!({"incremental": true, "changed": changes.len(), "compacted": compacted}))
    } else {
        let index = RetrievalIndex::open(path)?;
        let mut documents = Vec::with_capacity(meta.keys.len());
        for row in 0..meta.keys.len() {
            token.check()?;
            documents.push(document(input, meta, ids, row, 1));
        }
        index.rebuild(&documents, 1)?;
        index.close()?;
        drop(index);
        let mut stats = LiveStats::default();
        for document in &documents {
            token.check()?;
            stats.insert(document);
        }
        stats.save(path)?;
        Ok(json!({"incremental": false, "changed": documents.len(), "compacted": false}))
    }
}
