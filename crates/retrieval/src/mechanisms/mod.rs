//! Read-only serving adapters, bounded retrieval and evidence-aware ranking.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
pub type SparseField = BTreeMap<u32, f64>;

mod cue_sensing;
pub mod dense;
mod fields;
mod graph;
pub mod lexical;
pub mod residual;
mod seams;
mod serving;
mod wave;

pub use cue_sensing::*;
pub use dense::{DenseGeneration, DenseMatch, VectorRecord};
pub use fields::*;
pub use graph::*;
pub use lexical::{LexicalDocument, LexicalGeneration, LexicalMatch};
pub use residual::*;
pub use seams::*;
pub use serving::*;
pub use wave::*;
