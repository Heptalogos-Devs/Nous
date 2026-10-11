// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, type JsonValue } from "@bufbuild/protobuf";
import { TemporalExtentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  CreateEpisodeRequestSchema,
  EpisodeMemberSchema,
  EpisodeMutationRequestSchema,
  ListEpisodeRevisionsRequestSchema,
  LinkEpisodeRevisionsRequestSchema,
  ListRequestSchema,
  ObjectRequestSchema,
  PageSchema,
  ReviseEpisodeRequestSchema,
  CognitiveRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { z } from "zod";
import { nonnegativeInt64, CliError, boundedInteger } from "../agent.js";
import { revisionBasis } from "../mutation-input.js";
import type { CliEnvironment } from "../runtime.js";

const reference = z.string().min(1).max(2048);
const basisInput = z.strictObject({
  ref: reference,
  role: z.enum(["direct", "interpretation", "contextual"]).default("direct"),
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
});
const memberInput = z.strictObject({
  ref: reference,
  role: z.string().min(1).max(128),
  ordinal: z.number().int().nonnegative().optional(),
});
const episodeInput = z.strictObject({
  track_key: z.string().min(1).max(256).optional(),
  title: z.string().max(512).optional(),
  parent_episode_revision: reference.optional(),
  experience_time: z.unknown().optional(),
  boundary_explanation: z.string().min(1).max(8192),
  members: z.array(memberInput).min(1).max(512),
  basis: z.array(basisInput).min(1).max(512),
});

export async function episodeCommands(
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
  const page = create(PageSchema, {
    pageSize: boundedInteger(values["page-size"], 1, 200, "--page-size"),
    pageToken: values["page-token"],
  });
  if (action === "list")
    return client.memory.listEpisodes(
      create(ListRequestSchema, {
        subjectId,
        page,
        status: values.status ?? "",
      }),
    );
  if (action === "get" || action === "revision") {
    const kind = action === "get" ? "episode" : "episode_revision";
    const ref = await resolveReference(
      required(argument, `${kind} reference`),
      kind,
    );
    const request = create(ObjectRequestSchema, { subjectId, id: ref.value });
    return action === "get"
      ? client.memory.getEpisode(request)
      : client.memory.getEpisodeRevision(request);
  }
  if (action === "history") {
    const ref = await resolveReference(
      required(argument, "Episode reference"),
      "episode",
    );
    return client.memory.listEpisodeRevisions(
      create(ListEpisodeRevisionsRequestSchema, {
        subjectId,
        episodeId: ref.value,
        page,
      }),
    );
  }
  if (action === "create" || action === "revise") {
    const parsed = episodeInput.parse(
      await requestPayload(values["request-file"]),
    );
    const operationId = values["operation-id"] ?? crypto.randomUUID();
    const basis = await Promise.all(
      parsed.basis.map((item) => revisionBasis(env, item)),
    );
    const members = await Promise.all(
      parsed.members.map(async (item, index) => {
        const ref = await resolveReference(item.ref);
        return create(EpisodeMemberSchema, {
          ordinal: item.ordinal ?? index,
          role: item.role,
          reference: create(CognitiveRefSchema, {
            kind: ref.kind,
            value: ref.value,
          }),
        });
      }),
    );
    const experienceTime = parsed.experience_time
      ? fromJson(TemporalExtentSchema, parsed.experience_time as JsonValue)
      : undefined;
    const parentEpisodeRevisionId = parsed.parent_episode_revision
      ? (
          await resolveReference(
            parsed.parent_episode_revision,
            "episode_revision",
          )
        ).value
      : undefined;
    if (action === "create") {
      const trackKey = required(parsed.track_key, "track_key in semantic TOML");
      return client.memory.createEpisode(
        create(CreateEpisodeRequestSchema, {
          operationId,
          subjectId,
          trackKey,
          title: parsed.title,
          parentEpisodeRevisionId,
          experienceTime,
          boundaryExplanation: parsed.boundary_explanation,
          members,
          basis,
        }),
      );
    }
    const ref = await resolveReference(
      required(argument, "Episode reference"),
      "episode",
    );
    return client.memory.reviseEpisode(
      create(ReviseEpisodeRequestSchema, {
        operationId,
        subjectId,
        episodeId: ref.value,
        expectedObjectEpoch: nonnegativeInt64(
          values["expected-epoch"],
          "--expected-epoch",
        ),
        intent: z
          .enum(["resegment", "reinterpret"])
          .parse(required(values.intent, "--intent resegment|reinterpret")),
        title: parsed.title,
        parentEpisodeRevisionId,
        experienceTime,
        boundaryExplanation: parsed.boundary_explanation,
        members,
        basis,
      }),
    );
  }
  if (action === "link") {
    const from = await resolveReference(
      required(values.from, "--from exact Episode revision"),
      "episode_revision",
    );
    const to = await resolveReference(
      required(values.to, "--to exact Episode revision"),
      "episode_revision",
    );
    return client.memory.linkEpisodeRevisions(
      create(LinkEpisodeRevisionsRequestSchema, {
        operationId: values["operation-id"] ?? crypto.randomUUID(),
        subjectId,
        fromRevisionId: from.value,
        toRevisionId: to.value,
        relation: required(values.relation, "--relation"),
      }),
    );
  }
  if (
    ["suppress", "restore", "withdraw", "reaccept", "purge"].includes(
      action ?? "",
    )
  ) {
    const ref = await resolveReference(
      required(argument, "Episode reference"),
      "episode",
    );
    const request = create(EpisodeMutationRequestSchema, {
      operationId: values["operation-id"] ?? crypto.randomUUID(),
      subjectId,
      episodeId: ref.value,
      expectedObjectEpoch: nonnegativeInt64(
        values["expected-epoch"],
        "--expected-epoch",
      ),
    });
    switch (action) {
      case "suppress":
        return client.memory.suppressEpisode(request);
      case "restore":
        return client.memory.restoreEpisode(request);
      case "withdraw":
        return client.memory.withdrawEpisode(request);
      case "reaccept":
        return client.memory.reacceptEpisode(request);
      case "purge":
        return client.memory.purgeEpisode(request);
    }
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use episode list|get|revision|history|create|revise|link|suppress|restore|withdraw|reaccept|purge",
  );
}
