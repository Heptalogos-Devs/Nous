// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { NousClient } from "./index.js";
/** Shared bounded read requests. Authority stays in the owner's trusted usage/context. */
export function readOnlyQuery(
  client: Pick<NousClient, "cognition">,
  request: Parameters<NousClient["cognition"]["query"]>[0],
  options?: Parameters<NousClient["cognition"]["query"]>[1],
) {
  if (
    !request.nousql?.trim() ||
    new TextEncoder().encode(request.nousql).length > 32768
  )
    throw new Error("NousQL requires bounded nonempty text<=32KiB");
  return client.cognition.query(request, options);
}
export function readOnlySources(
  client: Pick<NousClient, "cognition">,
  request: Parameters<NousClient["cognition"]["readSources"]>[0],
  options?: Parameters<NousClient["cognition"]["readSources"]>[1],
) {
  if (
    !(request.references?.length || request.materialSources?.length) ||
    (request.references?.length ?? 0) + (request.materialSources?.length ?? 0) >
      128 ||
    (request.maxTextBytes ?? 0) > 1048576
  )
    throw new Error("Source read requires1..128 exact refs and<=1MiB text");
  return client.cognition.readSources(request, options);
}
