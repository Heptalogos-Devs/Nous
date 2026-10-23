use anyhow::Result;
use clap::Subcommand;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Subcommand)]
pub enum BundleCommand {
    /// Validate a local A-Memorix bundle and list its content and required owners.
    Inspect {
        #[arg(long)]
        file: PathBuf,
        /// Include the complete parsed records, references and member inventory.
        #[arg(long)]
        details: bool,
    },
}

pub fn inspect(file: &Path, details: bool) -> Result<Value> {
    let inspection = nous_app::bundle_format::inspect_file(file)?;
    if details {
        return Ok(serde_json::to_value(inspection)?);
    }
    let state_tables: BTreeMap<_, _> = inspection
        .state
        .as_ref()
        .map(|state| {
            state
                .tables
                .iter()
                .map(|(table, rows)| (table, rows.len()))
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "status": "validated",
        "manifest": inspection.manifest,
        "archive": {"sha256": inspection.archive_sha256, "bytes": inspection.archive_bytes,
            "expanded_bytes": inspection.expanded_bytes, "members": inspection.members.len()},
        "contents": {"documents": inspection.knowledge.docs.len(), "state_tables": state_tables,
            "image_assets": inspection.index.image_assets.len(),
            "image_occurrences": inspection.index.image_occurrences.len(),
            "vector_members": inspection.index.vector_members},
        "required_capabilities": inspection.required_capabilities
    }))
}
