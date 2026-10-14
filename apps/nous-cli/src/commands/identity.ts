// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { RebindEntityRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { content } from "../content.js";
import { CliError, uniqueReference } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function identityCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  const { client, values, required } = env;
  const subjectId = await env.subjectId;
  if (action === "rebind")
    return client.identity.rebindEntity({
      ...(await content(env, RebindEntityRequestSchema)),
      subjectId,
    });
  if (action === "resolve") {
    const result = await client.identity.resolveIdentity({
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
  }
  if (action === "bind")
    return client.identity.bindIdentity({
      subjectId,
      canonical: await env.resolveReference(
        required(values.canonical, "--canonical"),
        required(values.kind, "--kind"),
      ),
      displayName: required(values.name, "--name"),
      aliases: values.alias ?? [],
    });

  throw new CliError("INVALID_ARGUMENT", "Unknown identity command");
}
