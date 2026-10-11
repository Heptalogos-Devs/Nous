// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::ranges;
#[test]
fn segmentation_preserves_utf8_bytes_and_line_order() {
    let text = format!("first\n\n{}\nlast", "语音🙂".repeat(600));
    let spans = ranges(&text, 2048).unwrap();
    assert_eq!(
        spans.iter().map(|(a, b)| &text[*a..*b]).collect::<String>(),
        text
    );
    assert!(spans.iter().all(|(a, b)| b - a <= 2048));
    assert_eq!(&text[spans[0].0..spans[0].1], "first\n");
    assert_eq!(spans, ranges(&text, 2048).unwrap());
}
