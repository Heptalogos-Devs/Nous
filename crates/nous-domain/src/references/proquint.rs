//! Standard Proquint, three MSB-first 16-bit groups for a 48-bit address.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{Error, Result};

const CONSONANTS: &[u8; 16] = b"bdfghjklmnprstvz";
const VOWELS: &[u8; 4] = b"aiou";
const MAX_ADDRESS: u64 = (1 << 48) - 1;

pub fn encode_proquint(value: u64) -> Result<String> {
    if value > MAX_ADDRESS {
        return Err(Error::invalid("Proquint address exceeds 48 bits"));
    }
    let mut output = String::with_capacity(17);
    for shift in [32, 16, 0] {
        if !output.is_empty() {
            output.push('-');
        }
        let group = (value >> shift) as u16;
        for byte in [
            CONSONANTS[usize::from((group >> 12) & 15)],
            VOWELS[usize::from((group >> 10) & 3)],
            CONSONANTS[usize::from((group >> 6) & 15)],
            VOWELS[usize::from((group >> 4) & 3)],
            CONSONANTS[usize::from(group & 15)],
        ] {
            output.push(char::from(byte));
        }
    }
    Ok(output)
}

pub fn decode_proquint(value: &str) -> Result<u64> {
    let invalid = || Error::invalid("invalid readable reference");
    let bytes = value.as_bytes();
    if bytes.len() != 17 || bytes[5] != b'-' || bytes[11] != b'-' {
        return Err(invalid());
    }
    let mut result = 0u64;
    for start in [0, 6, 12] {
        let mut group = 0u16;
        for (offset, alphabet, width) in [
            (0, CONSONANTS.as_slice(), 4),
            (1, VOWELS.as_slice(), 2),
            (2, CONSONANTS.as_slice(), 4),
            (3, VOWELS.as_slice(), 2),
            (4, CONSONANTS.as_slice(), 4),
        ] {
            let index = alphabet
                .iter()
                .position(|&byte| byte == bytes[start + offset])
                .ok_or_else(invalid)?;
            group = (group << width) | index as u16;
        }
        result = (result << 16) | u64::from(group);
    }
    Ok(result)
}
