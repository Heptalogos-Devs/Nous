// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError, boundedInteger } from "../agent.js";
import { CreateAssociationRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { content } from "../content.js";
import type { CliEnvironment } from "../runtime.js";

export async function associationCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, values, required, resolveReference } = env;
  const subjectId = await env.subjectId;
  if (action === "revoke") {
    const operationId = values["operation-id"] ?? crypto.randomUUID();
    const associationId = (
      await resolveReference(
        required(argument, "Association reference"),
        "association",
      )
    ).value;
    await client.concepts.revokeAssociation({
      subjectId,
      operationId,
      associationId,
    });
    return { operationId, associationId, status: "revoked" };
  }
  if (action === "neighborhood") {
    const text = required(argument, "Reference");
    const kind = values.kind ?? "";
    const root = await resolveReference(text, kind);
    return client.concepts.getNeighborhood({
      subjectId,
      root,
      maxNodes: boundedInteger(values["max-nodes"], 1, 256, "--max-nodes"),
      maxDepth: boundedInteger(values["max-depth"], 1, 4, "--max-depth"),
    });
  }
  if (action === "create") {
    return client.concepts.createAssociation({
      ...(await content(env, CreateAssociationRequestSchema)),
      subjectId,
      operationId: values["operation-id"] ?? crypto.randomUUID(),
    });
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown association command");
}
