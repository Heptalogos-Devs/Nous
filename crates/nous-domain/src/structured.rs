//! Typed cognition: content, evidence, revisions and maintenance operations.
use crate::identity::validate_uuid;
use crate::{
    CognitiveId, CognitiveTimes, Error, MaterialId, MemoryId, ModelCallId, NamespaceId,
    ObservationId, Result, RetrievalTarget, SemanticTarget, SubjectId, TimeRange,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod content;
mod entity;
mod identity;
mod lineage;
mod objects;
mod proposal;
mod sources;
mod validation;
pub use content::*;
pub use entity::*;
pub use identity::*;
pub use lineage::*;
pub use objects::*;
pub use proposal::*;
use sources::dedup_links;
pub use sources::*;
use validation::*;
