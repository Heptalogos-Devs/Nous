// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { canonicalDigest } from "../../src/digest.js";
import { modelExecutionSchema } from "../../src/model/configuration.js";
import { modelImplementations } from "../../src/model/implementation.js";
import type { ModelRoleSnapshot } from "../../src/model/execution/snapshot.js";
import type { ModelRole } from "../../src/model/roles.js";

/** Durable shape for workflow tests whose model invocation is stubbed. */
export function modelRoleSnapshotFixture(role: ModelRole): ModelRoleSnapshot {
  const digest = canonicalDigest({ fixture: "model-role-snapshot", role });
  return {
    format: "nous.model.execution",
    implementationDigest: modelImplementations.provider,
    configuration: modelExecutionSchema.parse({}),
    role,
    profileDigest: digest,
    configDigest: digest,
  };
}
