// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commaSeparatedArgs, commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { CliError, uniqueReference } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { z } from "zod";

const rebindInput = z.strictObject({
  mentionId: z.string().uuid(),
  bindingState: z.enum(["bound", "unbound", "disputed"]),
  entity: z.string().min(1).max(2048).optional(),
  hostResolutionRef: z.string().max(2048).optional(),
  reason: z.string().max(2048).optional(),
});

const identityResolve = async (env: CliEnvironment) => {
  const { client, values, subjectId, required } = env;
  const result = await client.identity.resolve({
    subjectId,
    kind: values.kind ?? "",
    locator: values["lexical-ref"]
      ? { case: "lexicalRef", value: values["lexical-ref"] }
      : {
          case: "name",
          value: required(values.name, "--name or --lexical-ref"),
        },
  });
  uniqueReference(result);
  return result;
};
const identityBind = async (env: CliEnvironment) => {
  const { client, values, subjectId, required } = env;
  return client.identity.bind({
    subjectId,
    canonical: await env.resolveReference(
      required(values.canonical, "--canonical"),
      required(values.kind, "--kind"),
    ),
    displayName: required(values.name, "--name"),
    aliases: values.alias ?? [],
  });
};
const identityRebind = async (env: CliEnvironment, argument?: string) => {
  const { client, values, subjectId, required } = env;
  const input = rebindInput.parse({
    mentionId: required(argument, "Mention UUID"),
    bindingState: required(values["binding-state"], "--binding-state"),
    ...(values["entity-ref"] ? { entity: values["entity-ref"] } : {}),
    ...(values["host-resolution-ref"]
      ? { hostResolutionRef: values["host-resolution-ref"] }
      : {}),
    ...(values.reason ? { reason: values.reason } : {}),
  });
  const entityRef = input.entity
    ? (await env.resolveReference(input.entity, "entity")).value
    : undefined;
  if (input.bindingState === "bound" && !entityRef)
    throw new CliError(
      "INVALID_ARGUMENT",
      "--entity-ref is required when --binding-state is bound",
    );
  return client.identity.rebindEntity({
    subjectId,
    mentionId: input.mentionId,
    entityRef,
    bindingState: input.bindingState,
    hostResolutionRef: input.hostResolutionRef,
    reason: input.reason,
  });
};

export function createIdentityCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      bind: leaf(
        {
          ...stringArgs("kind", "canonical", "name"),
          ...commaSeparatedArgs("alias"),
        },
        identityBind,
      ),
      resolve: leaf(
        { ...stringArgs("kind", "name", "lexical-ref") },
        identityResolve,
      ),
      rebind: leaf(
        {
          ...stringArgs(
            "binding-state",
            "entity-ref",
            "host-resolution-ref",
            "reason",
          ),
        },
        identityRebind,
        {
          description:
            "Revise an entity mention binding with an explicit state.",
          parameters: ["<Mention UUID>"],
        },
      ),
    },
  });
}
