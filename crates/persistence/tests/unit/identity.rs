// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn standard_reference_validation() {
    assert_eq!(validate_lexical("mem:bahog-hijol-mokor").unwrap(), "mem");
    assert!(validate_lexical("Mem:bahog-hijol-mokor").is_err());
    assert!(validate_lexical("unknown:bahog-hijol-mokor").is_err());
    assert!(validate_lexical("mem:bahog-hijol").is_err());
}
