//! Typed cognition: content, evidence, revisions and maintenance operations.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see UPSTREAM.md.
use crate::identity::validate_uuid;
use crate::{
    CognitiveId, CognitiveTimes, Error, MaterialId, MemoryId, ModelCallId, NamespaceId,
    ObservationId, Result, SubjectId, TimeRange,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod content;
mod identity;
mod lineage;
mod objects;
mod proposal;
mod sources;
mod validation;
pub use content::*;
pub use identity::*;
pub use lineage::*;
pub use objects::*;
pub use proposal::*;
use sources::dedup_links;
pub use sources::*;
use validation::*;
