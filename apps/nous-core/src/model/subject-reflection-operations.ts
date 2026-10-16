// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { ServiceImpl } from "@connectrpc/connect";
import { SubjectReflectionService } from "@nous-wave/protocol/nous/wave/v1alpha1/reflection_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { repairSubjectReflection } from "./subject-reflection.js";
export function reflectionOperations(
  kernel: KernelClient,
): ServiceImpl<typeof SubjectReflectionService> {
  return {
    inspect: (r, c) =>
      kernel.maintenance.inspectSubjectReflection(r, {
        signal: c.signal,
        timeoutMs: c.timeoutMs(),
      }),
    repair: (r, c) =>
      repairSubjectReflection(kernel, r, {
        signal: c.signal,
        timeoutMs: c.timeoutMs(),
      }),
  };
}
