// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { developmentLocator } from "../workspace.js";

const located = process.argv
  .slice(2)
  .some((arg) => /^--(?:home|locator)(?:=|$)/.test(arg));
process.argv.splice(
  2,
  0,
  "--development",
  ...(located ? [] : ["--locator", await developmentLocator()]),
);
await import("../../apps/nous-core/src/launcher.js");
