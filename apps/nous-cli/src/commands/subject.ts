// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  CognitiveSeedSchema,
  CreateSubjectRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { content, operationId, page } from "../content.js";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function subjectCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, save, required } = env;
  if (action === "list")
    return client.subjects.listSubjects({
      status: env.values.status,
      page: page(env),
    });
  if (action === "create") {
    const subject = await client.subjects.createSubject(
      env.values["request-file"]
        ? {
            ...(await content(env, CreateSubjectRequestSchema)),
            operationId: operationId(env),
          }
        : {
            operationId: operationId(env),
            capabilities: { memory: true },
            cognitiveSeed: {
              text: "schema_version = 1",
              format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
            },
          },
    );
    await save({
      format: "nous.consumer.selection",
      subjectId: subject.subjectId,
    });
    return subject;
  }
  if (action === "seed")
    return client.subjects.getCognitiveSeed({
      subjectId: required(await env.subjectId, "Selected Subject"),
    });
  if (action === "adopt-seed")
    return client.subjects.adoptCognitiveSeed({
      subjectId: required(await env.subjectId, "Selected Subject"),
      operationId: operationId(env),
      kind: env.values.kind ?? "import",
      seed: await content(env, CognitiveSeedSchema),
    });
  if (action === "use") {
    const subject = await client.subjects.getSubject({
      subjectId: (
        await env.resolveReference(
          required(argument, "Subject reference"),
          "subject",
        )
      ).value,
    });
    await save({
      format: "nous.consumer.selection",
      subjectId: subject.subjectId,
    });
    return subject;
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown subject command");
}
