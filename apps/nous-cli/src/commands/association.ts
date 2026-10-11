// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { boundedInteger } from "../agent.js";
import { associationInput, associationBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

const associationRevoke = async (env: CliEnvironment, argument?: string) => {
  const { client, values, subjectId, required, resolveReference } = env;
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
};
const associationNeighborhood = async (
  env: CliEnvironment,
  argument?: string,
) => {
  const { client, values, subjectId, required, resolveReference } = env;
  const text = required(argument, "Reference");
  const kind = values.kind ?? "";
  const root = await resolveReference(text, kind);
  return client.concepts.neighborhood({
    subjectId,
    root,
    maxNodes: boundedInteger(values["max-nodes"], 1, 256, "--max-nodes"),
    maxDepth: boundedInteger(values["max-depth"], 1, 4, "--max-depth"),
  });
};
const associationCreate = async (env: CliEnvironment) => {
  const { client, values, subjectId, resolveReference, requestPayload } = env;
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
};

export function createAssociationCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      create: leaf(
        { ...stringArgs("association-file", "operation-id") },
        associationCreate,
      ),
      revoke: leaf({ ...stringArgs("operation-id") }, associationRevoke, {
        parameters: ["<Association reference>"],
      }),
      neighborhood: leaf(
        { ...stringArgs("kind", "max-nodes", "max-depth") },
        associationNeighborhood,
        { parameters: ["<reference>"] },
      ),
    },
  });
}
