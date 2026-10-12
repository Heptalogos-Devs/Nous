// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { SystemStatus } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import type { CliEnvironment } from "../runtime.js";
export async function statusCommands(
  env: CliEnvironment,
  snapshot?: SystemStatus,
) {
  const status = snapshot ?? (await env.client.system.getStatus({}));
  const kernelReady = status.components.some(
    (component) => component.name === "kernel" && component.state === "READY",
  );
  return {
    status,
    ...(env.subjectId && kernelReady
      ? {
          subjectId: env.subjectId,
          serving: await env.client.system.getProjectionStatus({
            subjectId: env.subjectId,
          }),
        }
      : {}),
  };
}
