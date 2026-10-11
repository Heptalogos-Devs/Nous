// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createHash } from "node:crypto";
import type { MaintenanceNeed } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";

function operationIdentity(key: string) {
  const bytes = createHash("sha1")
    .update(Buffer.from("6ba7b8129dad11d180b400c04fd430c8", "hex"))
    .update(key)
    .digest()
    .subarray(0, 16);
  bytes[6] = (bytes[6]! & 15) | 80;
  bytes[8] = (bytes[8]! & 63) | 128;
  const value = bytes.toString("hex");
  return `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`;
}
export function maintenanceOperationId(need: MaintenanceNeed) {
  return operationIdentity(
    `${need.needId}:${need.triggerAuthoritySeq}:${need.triggerRevision}`,
  );
}
export function maintenanceActionOperationId(
  operation: string,
  index: number,
  kind: string,
) {
  return operationIdentity(`${operation}:${index}:${kind}`);
}
