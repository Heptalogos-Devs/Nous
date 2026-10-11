// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError, uniqueReference } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function identityCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
  const { client, values, subjectId, required } = env;
  if (action === "resolve") {
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
  }
  if (action === "bind")
    return client.identity.bind({
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
