// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { commaSeparatedArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import type { CliEnvironment } from "../runtime.js";

async function formCommands(env: CliEnvironment, argument?: string) {
  const { client, values, subjectId, required, resolveReference } = env;
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  const result = await client.model.formFromObservation({
    subjectId,
    operationId,
    aboutnessMode:
      values["aboutness-mode"] ??
      (values.aboutness?.length ? "explicit" : undefined),
    explicitAboutness: values.aboutness,
    explicitTags: await Promise.all(
      (values.tag ?? []).map(
        async (text) => (await resolveReference(text, "tag")).value,
      ),
    ),
    occurrenceId: (
      await resolveReference(
        required(argument, "Occurrence reference"),
        "occurrence",
      )
    ).value,
    representationId: values.representation
      ? (
          await resolveReference(
            values.representation,
            "derived_representation",
          )
        ).value
      : undefined,
  });
  return { ...result, operationId };
}

export function createFormCommand(leaf: CliLeaf) {
  return leaf(
    {
      ...stringArgs("operation-id", "aboutness-mode", "representation"),
      ...commaSeparatedArgs("aboutness", "tag"),
    },
    formCommands,
    {
      description: "Form cognition from an admitted Observation.",
      parameters: ["<Occurrence reference>"],
    },
  );
}
