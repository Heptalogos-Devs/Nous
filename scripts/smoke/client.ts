// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { rm } from "node:fs/promises";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";
import { verifyClient } from "../release/verify-client.js";
import { workspaceTemp } from "../workspace.js";
import { boot, prepare, stop, type Boot } from "./support.js";

const root = await workspaceTemp("smoke", "client-");
let current: Boot | undefined;
let passed = false;
try {
  const locator = await prepare(root);
  current = await boot(locator);
  const locations = await resolveLocations({
    locator,
    installationHome: process.cwd(),
  });
  await verifyClient(locations.run);
  passed = true;
} finally {
  await stop(current);
  if (passed) await rm(root, { recursive: true, force: true });
}
