// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { operationIdentity } from "../operation-identity.js";
import type { MaintenanceNeed } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";

export function maintenanceOperationId(need: MaintenanceNeed) {
  return operationIdentity(
    `${need.needId}:${need.triggerAuthoritySeq}:${need.triggerRevision}`,
  );
}
export function maintenanceActionOperationId(
  workflow: string,
  index: number,
  kind: string,
) {
  return operationIdentity(`${workflow}:${index}:${kind}`);
}
