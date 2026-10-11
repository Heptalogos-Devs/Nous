// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { stringify } from "smol-toml";
import { CONFIG_REVISION, parseConfiguration } from "./schema.js";
import type { RuntimeLocations } from "../locations.js";

/** Create operator-owned configuration once; schema defaults own runtime policy. */
export async function initializeConfiguration(locations: RuntimeLocations) {
  const path = join(locations.config, "nous.toml");
  await mkdir(locations.config, { recursive: true });
  const initial = stringify({
    config_revision: CONFIG_REVISION,
    consumers: [{ consumer_id: "default" }],
  });
  parseConfiguration(initial);
  try {
    await writeFile(path, initial, { flag: "wx", mode: 0o600 });
    return { path, created: true };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
    return { path, created: false };
  }
}
