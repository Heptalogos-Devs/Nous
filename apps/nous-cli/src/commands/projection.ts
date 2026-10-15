// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  ContextCursorSchema,
  CognitionUsageSchema,
  QueryExprSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { boundedInteger } from "../agent.js";
import { content } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function projectionCommands(env: CliEnvironment, action?: string) {
  const file = env.values["request-file"]
    ? ((await env.requestPayload(env.values["request-file"])) as {
        query?: unknown;
        situation_refs?: string[];
        known_cursor?: unknown;
        usage?: unknown;
      })
    : {};
  const projection = {
    subjectId: await env.subjectId,
    sessionId: env.required(await env.sessionId, "Selected Session"),
    consumerId: env.required(env.values.consumer, "--consumer"),
    maxItems: boundedInteger(
      env.values["max-items"] ?? "16",
      1,
      256,
      "--max-items",
    ),
    maxTextBytes: boundedInteger(
      env.values["max-bytes"] ?? "65536",
      1,
      1048576,
      "--max-bytes",
    ),
    query: file.query
      ? await content(env, QueryExprSchema, file.query)
      : undefined,
    situationRefs: await Promise.all(
      (file.situation_refs ?? []).map((value) => env.resolveReference(value)),
    ),
    usage:
      action === "managed"
        ? await content(
            env,
            CognitionUsageSchema,
            file.usage ?? {
              agentInstanceId: `cli:${env.values.consumer}`,
              purpose: "reflection",
              intent:
                "Inspect the current cognition for the selected task and session.",
              audience: {
                conversationRef: env.required(
                  await env.sessionId,
                  "Selected Session",
                ),
                visibility: "internal",
              },
            },
          )
        : undefined,
  };
  return action === "managed"
    ? env.client.cognition.buildManagedContext({
        projection,
        knownCursor: file.known_cursor
          ? await content(env, ContextCursorSchema, file.known_cursor)
          : undefined,
      })
    : env.client.cognition.buildProjection(projection);
}
