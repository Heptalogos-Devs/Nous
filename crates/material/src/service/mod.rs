//! Immutable material/evidence registration and explicitly invoked derivation.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod artifact;
pub use artifact::ArtifactRegistration;
mod derivation;
mod documents;
mod historical;
mod identity;
mod lineage;
pub use documents::text_content_identity;
mod materialization;
mod observation;
mod query;
mod read;
mod segmentation;
mod structured;
pub use segmentation::DescriptionSegment;
mod types;
pub use materialization::{ByteRange, MaterialTextView, MaterializeRequest, MaterializedEvidence};
pub use read::MaterialReadInput;
pub use types::*;

use crate::*;
use chrono::{DateTime, Utc};
use futures::Stream;
use nous_core::*;
use nous_object_store::ObjectStore;
use nous_persistence::{AuthorityStore, ProjectionInvalidation};
use nous_runtime::{CognitiveRuntimeService, ResidentAdmission, ResidentState};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone)]
pub struct MaterialService {
    pub(crate) store: AuthorityStore,
    pub(crate) objects: ObjectStore,
    pub(crate) cognition: CognitiveRuntimeService,
    pub max_upload_bytes: u64,
}

impl MaterialService {
    pub fn new(
        store: AuthorityStore,
        objects: ObjectStore,
        cognition: CognitiveRuntimeService,
        max_upload_bytes: u64,
    ) -> Result<Self> {
        if max_upload_bytes == 0 {
            return Err(Error::Invalid("max_upload_bytes must be positive".into()));
        }
        Ok(Self {
            store,
            objects,
            cognition,
            max_upload_bytes,
        })
    }
}
