// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

process.argv.splice(2, 0, "serve");
await import("./nous.js");
