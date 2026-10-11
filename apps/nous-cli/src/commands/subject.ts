// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import {
  CognitiveSeedSchema,
  CreateSubjectRequestSchema,
  AdoptCognitiveSeedRequestSchema,
  SubjectRequestSchema,
  SubjectCapabilitiesSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";

const subjectInput = z.strictObject({
  cognitive_seed: z
    .strictObject({
      text: z.string().min(1).max(65536),
      format: z.string().min(1).max(256),
      provenance: z.record(z.string(), z.json()).optional(),
    })
    .optional(),
  metadata: z.record(z.string(), z.json()).optional(),
  capabilities: z.strictObject({ memory: z.boolean() }).optional(),
});
const adoptionInput = z.strictObject({
  text: z.string().min(1).max(65536),
  format: z.string().min(1).max(256).optional(),
  provenance: z.record(z.string(), z.json()).optional(),
  kind: z.enum(["initial", "import"]),
});

export async function subjectCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, save, required, values, requestPayload, resolveReference } =
    env;
  if (action === "list") return client.subjects.list({});
  if (action === "get" || action === "seed" || action === "adopt-seed") {
    const subjectId = (
      await resolveReference(required(argument, "Subject reference"), "subject")
    ).value;
    if (action === "get")
      return client.subjects.get(create(SubjectRequestSchema, { subjectId }));
    if (action === "seed")
      return client.subjects.seed(create(SubjectRequestSchema, { subjectId }));
    const input = adoptionInput.parse(
      await requestPayload(values["request-file"]),
    );
    return client.subjects.adoptSeed(
      create(AdoptCognitiveSeedRequestSchema, {
        subjectId,
        operationId: values["operation-id"] ?? crypto.randomUUID(),
        seed: create(CognitiveSeedSchema, {
          text: input.text,
          format:
            input.format ??
            "application/vnd.nous-wave.cognitive-seed+toml;version=1",
          provenance: input.provenance,
        }),
        kind: input.kind,
      }),
    );
  }
  if (action === "create") {
    const request = values["request-file"]
      ? subjectInput.parse(await requestPayload(values["request-file"]))
      : {};
    const subject = await client.subjects.create(
      create(CreateSubjectRequestSchema, {
        subjectId: crypto.randomUUID(),
        operationId: crypto.randomUUID(),
        ...(request.capabilities
          ? {
              capabilities: create(
                SubjectCapabilitiesSchema,
                request.capabilities,
              ),
            }
          : {}),
        ...(request.cognitive_seed
          ? {
              cognitiveSeed: create(CognitiveSeedSchema, {
                text: request.cognitive_seed.text,
                format: request.cognitive_seed.format,
                provenance: request.cognitive_seed.provenance,
              }),
            }
          : {}),
        ...(request.metadata ? { metadata: request.metadata } : {}),
      }),
    );
    await save({
      format: "nous.consumer.selection",
      subjectId: subject.subjectId,
    });
    return subject;
  }
  if (action === "use") {
    const subject = await client.subjects.get({
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
