// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError, boundedInteger } from "../agent.js";
import { associationInput, associationBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

export async function associationCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const {
    client,
    values,
    subjectId,
    required,
    resolveReference,
    requestPayload,
  } = env;
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
    return client.concepts.neighborhood({
      subjectId,
      root,
      maxNodes: boundedInteger(values["max-nodes"], 1, 256, "--max-nodes"),
      maxDepth: boundedInteger(values["max-depth"], 1, 4, "--max-depth"),
    });
  }
  if (action === "create") {
    const input = associationInput.parse(
      await requestPayload(values["association-file"]),
    );
    return client.concepts.associate({
      subjectId,
      operationId: values["operation-id"] ?? crypto.randomUUID(),
      association: {
        from: await resolveReference(input.from),
        to: await resolveReference(input.to),
        relationKind: input.relation,
        polarity: input.polarity,
        basisClass: input.basis_class,
        basis: await Promise.all(
          input.basis.map((entry) => associationBasis(env, entry)),
        ),
      },
    });
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown association command");
}
