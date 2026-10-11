// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  MemoryContentSchema,
  MemoryHistoryRequestSchema,
  MemoryMutationRequestSchema,
  LinkRevisionsRequestSchema,
  ListRequestSchema,
  ObjectRequestSchema,
  PageSchema,
  ReviseMemoryRequestSchema,
  SetAccessibilityRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";
import { nonnegativeInt64, CliError, boundedInteger } from "../agent.js";
import { revisionBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

const semanticMemory = z
  .strictObject({
    intent: z.enum(["correct", "rephrase", "reinterpret"]),
    cognitive_role: z.enum([
      "experiential",
      "declarative",
      "procedural_experience",
    ]),
    formation_mode: z.enum(["grounded", "synthesized"]),
    grounding_occurrence: z.string().min(1).max(2048).optional(),
    semantic_role: z.string().min(1).max(128),
    text: z.string().min(1).max(65536),
    title: z.string().max(512).optional(),
    basis: z
      .array(
        z.strictObject({
          ref: z.string().min(1).max(2048),
          role: z
            .enum(["direct", "interpretation", "contextual"])
            .default("direct"),
          epistemic_relation: z
            .enum([
              "supports",
              "contradicts",
              "corroborates",
              "weakens",
              "corrects",
              "counterexample",
              "inferred_from",
            ])
            .optional(),
        }),
      )
      .min(1)
      .max(256),
    aboutness: z.array(z.string().min(1).max(2048)).max(256).default([]),
    tags: z.array(z.string().min(1).max(2048)).max(256).default([]),
    valid_time: z.unknown().optional(),
    epistemic_class: z.enum([
      "observed",
      "reported",
      "derived",
      "inferred",
      "narrative",
      "simulated",
    ]),
  })
  .strict();

export async function memoryCommands(
  env: CliEnvironment,
  action?: string,
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
  if (action === "list")
    return client.memory.list(
      create(ListRequestSchema, {
        subjectId,
        page: create(PageSchema, {
          pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
          pageToken: values["page-token"],
        }),
        status: values.status ?? "",
      }),
    );
  if (action === "get" || action === "revision") {
    const kind = action === "get" ? "memory" : "memory_revision";
    const ref = await resolveReference(
      required(argument, `${kind} reference`),
      kind,
    );
    const request = create(ObjectRequestSchema, { subjectId, id: ref.value });
    return action === "get"
      ? client.memory.get(request)
      : client.memory.revision(request);
  }
  if (action === "history") {
    const ref = await resolveReference(
      required(argument, "memory reference"),
      "memory",
    );
    const page = create(PageSchema, {
      pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
      pageToken: values["page-token"],
    });
    return client.memory.history(
      create(MemoryHistoryRequestSchema, {
        subjectId,
        memoryId: ref.value,
        page,
      }),
    );
  }
  if (action === "revise") {
    const ref = await resolveReference(
      required(argument, "memory reference"),
      "memory",
    );
    const parsed = semanticMemory.parse(
      await requestPayload(values["request-file"]),
    );
    const expectedObjectEpoch = nonnegativeInt64(
      values["expected-epoch"],
      "--expected-epoch",
    );
    const basis = await Promise.all(
      parsed.basis.map((item) => revisionBasis(env, item)),
    );
    const aboutness = await Promise.all(
      parsed.aboutness.map(
        async (text) => (await resolveReference(text, "entity")).value,
      ),
    );
    const tags = await Promise.all(
      parsed.tags.map(
        async (text) => (await resolveReference(text, "tag")).value,
      ),
    );
    const groundingOccurrenceId = parsed.grounding_occurrence
      ? (await resolveReference(parsed.grounding_occurrence, "occurrence"))
          .value
      : undefined;
    const validTime = parsed.valid_time
      ? fromJson(TemporalExtentSchema, parsed.valid_time as JsonValue)
      : undefined;
    const input = create(MemoryContentSchema, {
      cognitiveRole: parsed.cognitive_role,
      formationMode: parsed.formation_mode,
      groundingOccurrenceId,
      semanticRole: parsed.semantic_role,
      text: parsed.text,
      title: parsed.title,
      basis,
      aboutness,
      tags,
      validTime,
      epistemicClass: parsed.epistemic_class,
    });
    return client.memory.revise(
      create(ReviseMemoryRequestSchema, {
        operationId: values["operation-id"] ?? crypto.randomUUID(),
        subjectId,
        memoryId: ref.value,
        expectedObjectEpoch,
        input,
        intent: parsed.intent,
      }),
    );
  }
  if (
    ["suppress", "restore", "withdraw", "reaccept", "purge"].includes(
      action ?? "",
    )
  ) {
    const ref = await resolveReference(
      required(argument, "Memory reference"),
      "memory",
    );
    const epoch = nonnegativeInt64(
      values["expected-epoch"],
      "--expected-epoch",
    );
    const request = create(MemoryMutationRequestSchema, {
      operationId: values["operation-id"] ?? crypto.randomUUID(),
      subjectId,
      memoryId: ref.value,
      expectedObjectEpoch: epoch,
    });
    switch (action) {
      case "suppress":
        return client.memory.suppress(request);
      case "restore":
        return client.memory.restore(request);
      case "withdraw":
        return client.memory.withdraw(request);
      case "reaccept":
        return client.memory.reaccept(request);
      case "purge":
        return client.memory.purge(request);
    }
  }
  if (action === "accessibility") {
    const ref = await resolveReference(
      required(argument, "Memory reference"),
      "memory",
    );
    const epoch = nonnegativeInt64(
      values["expected-epoch"],
      "--expected-epoch",
    );
    const mode = z
      .enum(["auto", "normal", "deep", "explicit"])
      .parse(required(values.mode, "--mode auto|normal|deep|explicit"));
    return client.memory.setAccessibility(
      create(SetAccessibilityRequestSchema, {
        operationId: values["operation-id"] ?? crypto.randomUUID(),
        subjectId,
        memoryId: ref.value,
        expectedObjectEpoch: epoch,
        mode,
      }),
    );
  }
  if (action === "link") {
    const from = await resolveReference(
      required(values.from, "--from exact Memory revision"),
      "memory_revision",
    );
    const to = await resolveReference(
      required(values.to, "--to exact Memory revision"),
      "memory_revision",
    );
    return client.memory.linkRevisions(
      create(LinkRevisionsRequestSchema, {
        operationId: values["operation-id"] ?? crypto.randomUUID(),
        subjectId,
        fromRevisionId: from.value,
        toRevisionId: to.value,
        relation: required(values.relation, "--relation"),
      }),
    );
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use memory list|get|revision|history|revise|suppress|restore|withdraw|reaccept|purge|accessibility|link",
  );
}
