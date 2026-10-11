// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { CliEnvironment } from "../runtime.js";

export async function formCommands(
  env: CliEnvironment,
  action: string | undefined,
) {
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
        required(action, "Occurrence reference"),
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
