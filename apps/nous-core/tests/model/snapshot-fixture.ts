// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { canonicalDigest } from "../../src/digest.js";
import { modelExecutionSchema } from "../../src/model/configuration.js";
import type { ModelRoleSnapshot } from "../../src/model/execution/snapshot.js";
import type { ModelRole } from "../../src/model/roles.js";

/** In-memory role configuration for stubbed model calls. */
export function modelRoleSnapshotFixture(role: ModelRole): ModelRoleSnapshot {
  const digest = canonicalDigest({ fixture: "model-role-snapshot", role });
  return {
    configuration: modelExecutionSchema.parse({}),
    role,
    profileDigest: digest,
    configDigest: digest,
  };
}
