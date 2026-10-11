// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

mod eligibility;
mod expression;
mod request;
mod result;
pub use eligibility::QueryFacts;
pub use expression::*;
pub use request::*;
pub use result::*;

#[cfg(test)]
#[path = "../../tests/unit/query_projection.rs"]
mod projection_tests;
