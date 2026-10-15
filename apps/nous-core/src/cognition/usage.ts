// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  CueSchema,
  QueryExprSchema,
  type ProjectionRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

/** Bind actual invocation context into the existing query, without issuing rights. */
export function bindProjectionUsage(request: ProjectionRequest): void {
  const usage = request.usage;
  if (!usage) return;
  const bounded = (value: string, maximum: number) =>
    value.trim().length > 0 && Buffer.byteLength(value) <= maximum;
  if (
    !bounded(usage.agentInstanceId, 512) ||
    !["deliberation", "expression", "reflection", "worker"].includes(
      usage.purpose,
    ) ||
    !bounded(usage.intent, 8192) ||
    (usage.activityRef !== undefined && !bounded(usage.activityRef, 1024)) ||
    usage.participants.length > 64 ||
    usage.participants.some(
      (participant) =>
        !bounded(participant.entityRef, 512) ||
        !["speaker", "addressee", "participant"].includes(participant.role),
    ) ||
    !usage.audience ||
    !bounded(usage.audience.conversationRef, 1024) ||
    !["direct", "group", "public", "internal"].includes(
      usage.audience.visibility,
    ) ||
    usage.audience.recipientEntityRefs.length > 64 ||
    usage.audience.recipientEntityRefs.some((value) => !bounded(value, 512))
  ) {
    throw new ConnectError(
      "Invalid cognition invocation usage",
      Code.InvalidArgument,
    );
  }
  const entities = new Set([
    ...usage.participants.map((participant) => participant.entityRef),
    ...usage.audience.recipientEntityRefs,
  ]);
  const usageQuery = create(QueryExprSchema, {
    operation: "atom",
    cues: [
      { cue: { case: "text", value: usage.intent } },
      ...[...entities].map((value) =>
        create(CueSchema, {
          cue: { case: "entityRef", value },
        }),
      ),
    ],
  });
  request.query = request.query
    ? create(QueryExprSchema, {
        operation: "union",
        children: [request.query, usageQuery],
      })
    : usageQuery;
}
