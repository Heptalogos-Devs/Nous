//! Independently implemented numerical contracts extracted from frozen VCP.
//! Neutral DTOs have no Authority, MemoryRevision or AssociationEvidence owner.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod fields;
pub use fields::*;
mod sense;
pub use sense::*;
mod graph;
pub use graph::*;
mod pyramid;
pub use pyramid::*;
mod query_shape;
pub use query_shape::*;
mod anchors;
pub(crate) use anchors::reference_cosine;
pub use anchors::*;
mod geometry;
pub use geometry::*;
mod topology;
pub use topology::*;
mod scoring;
pub use scoring::*;
mod observables;
pub use observables::*;
mod pool;
pub use pool::*;
mod readout;
pub use readout::*;
mod dtsc;
pub use dtsc::*;
mod intrinsic;
pub use intrinsic::*;
mod gating;
pub use gating::*;
mod fusion;
pub use fusion::*;
mod epa_training;
pub use epa_training::*;
mod pipeline;
pub use pipeline::*;

// Shared finite scalar policy for the frozen numerical kernels.
fn unit(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
