// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn standard_vectors_and_full_width_round_trip() {
    for (bits, text) in [
        (0, "babab-babab-babab"),
        (0x0123_4567_89ab, "bahog-hijol-mokor"),
        (MAX_ADDRESS, "zuzuz-zuzuz-zuzuz"),
    ] {
        assert_eq!(encode_proquint(bits).unwrap(), text);
        assert_eq!(decode_proquint(text).unwrap(), bits);
    }
    for bits in [1, 0xffff, 0x1_0000, 0x8000_0000_0000, 0xabcd_ef01_2345] {
        let text = encode_proquint(bits).unwrap();
        assert_eq!(text.len(), 17);
        assert_eq!(decode_proquint(&text).unwrap(), bits);
    }
    assert!(encode_proquint(1 << 48).is_err());
}

#[test]
fn rejects_nonstandard_spelling_and_grouping() {
    for value in [
        "Bahog-hijol-mokor",
        "bahog_hijol_mokor",
        "bahog-hijol",
        "bahog-hijol-mokor-babab",
        "0qbahog-hijol-mokor",
        "wahog-hijol-mokor",
        "behog-hijol-mokor",
        "bahog-hijol-mokoé",
    ] {
        assert!(decode_proquint(value).is_err(), "{value}");
    }
}
