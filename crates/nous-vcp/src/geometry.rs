//! Adapted from VCPToolBox rust-vexus-lite/src/memo_geometry.rs
//! Upstream commit d1b40c863273cbbba082fb7657671424768c4a53.
//! CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
//! Host/database access is replaced by immutable typed inputs.
//! Unified anchored greedy Gram-Schmidt geometry.
//! All floating-point reductions use ID order, never Rayon reduction order.

// Retain the upstream numerical decomposition and floating-point accumulation order.
#![allow(clippy::too_many_arguments)]
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

mod ags;
pub use ags::{AgsCandidate, AgsTarget, AgsTrace, anchored_gs, dot};
use ags::{direct_axes_core, dot_four, effective_k, normalize};
mod types;
use types::LocalResult;
pub use types::{GeometryInput, GeometryOutput, GeometryTag, IntrinsicResidual};
mod workspace;
use workspace::{
    UnitVectorWorkspace, accumulate_captured_energy, dot_neighbors, encode_vector, hash, number,
    semantic_weight, star_direction,
};
mod build;
pub use build::build_geometry;

const ALGORITHM: &str = "unified_geometry_ags_t4_cached_reorth";
