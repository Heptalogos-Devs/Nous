// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Public, Node-free wire authority. Canonical protobuf JSON is decoded by its
// generated descriptor at the consuming boundary, never a second cognition DTO.
import { z } from "zod";
export const HOST_SELECTED_MEDIA_CONTEXT_KEY = "host_selected_media";
export const HOST_ACTIVITY_CONTEXT_KEY = "activity_ref";
export const HOST_BRIDGE_PATHS = Object.freeze({
  prepare: "/nous/v1/prepare",
  admitted: "/nous/v1/admitted",
  handoff: "/nous/v1/handoff",
  feedback: "/nous/v1/feedback",
  toolExecution: "/nous/v1/tool-execution",
  completion: "/nous/v1/completion",
  runTerminal: "/gateway/v1/cognition/run-terminal",
  releaseResult: "/nous/v1/result-release",
});
const canonicalJson = z.record(z.string(), z.json());
export const nativeIdentitySchema = z
  .object({
    sessionKey: z.string().min(1),
    sessionId: z.string().min(1),
    runId: z.string().min(1),
    agentId: z.string().min(1),
    historyGeneration: z.string().min(1),
    parentSessionKey: z.string().min(1).optional(),
  })
  .strict();
const contactIdentity = {
  version: z.literal(1),
  contactRequestId: z.uuid(),
  native: nativeIdentitySchema,
  subjectId: z.uuid(),
  nousSessionId: z.uuid(),
  invocationId: z.string().min(1),
};
export const hostContactIdentitySchema = z.object(contactIdentity).strict();
export type HostContactIdentity = z.infer<typeof hostContactIdentitySchema>;
export const hostPrepareRequestSchema = z
  .object({
    ...contactIdentity,
    promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
    currentContextDigest: z
      .string()
      .regex(/^[a-f0-9]{64}$/)
      .optional(),
    historyUsers: z
      .array(
        z
          .object({
            key: z.string().min(1).max(4096),
            nativeRunId: z.string().min(1).max(2048),
            promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
          })
          .strict(),
      )
      .max(2048)
      .optional(),
  })
  .strict();
export const hostScopeSchema = z
  .object({
    activityRef: z.string().min(1),
    actorEntityRef: z.string().min(1).optional(),
    context: canonicalJson.default({}),
  })
  .strict();
/** Supplied by the authenticated caller/Host, never by model tool arguments. */
export const hostRoleScopeSchema = z
  .object({
    role: z.enum([
      "world.planner",
      "subject.deliberation",
      "subject.execution",
      "subject.render",
      "narrative.review",
      "subject.reflection",
      "system.maintenance",
    ]),
    purpose: z.enum([
      "world_authoring",
      "subject_deliberation",
      "professional_execution",
      "subject_rendering",
      "narrative_review",
      "reflection",
      "maintenance",
    ]),
    view: z.enum([
      "world_author",
      "subject",
      "worker",
      "renderer",
      "review",
      "reflection",
      "maintenance",
    ]),
    contactNature: z.enum(["execution_only", "subject_experience"]),
  })
  .strict()
  .refine((value) => {
    const expected: Record<string, readonly [string, string]> = {
      "world.planner": ["world_authoring", "world_author"],
      "subject.deliberation": ["subject_deliberation", "subject"],
      "subject.execution": ["professional_execution", "worker"],
      "subject.render": ["subject_rendering", "renderer"],
      "narrative.review": ["narrative_review", "review"],
      "subject.reflection": ["reflection", "reflection"],
      "system.maintenance": ["maintenance", "maintenance"],
    };
    const pair = expected[value.role];
    return pair?.[0] === value.purpose && pair[1] === value.view;
  }, "Role, purpose and cognitive view must describe the same trusted invocation");

export type HostRoleScope = z.infer<typeof hostRoleScopeSchema>;

/** Trusted per-attempt bounds over the complete native input and provider output. */
export const hostTokenBudgetSchema = z
  .object({
    inputTokens: z.number().int().positive(),
    outputTokens: z.number().int().positive(),
  })
  .strict();
export type HostTokenBudget = z.infer<typeof hostTokenBudgetSchema>;
/** Exact native structured-output control supplied by the trusted role owner. */
export const hostResponseFormatSchema = z.union([
  z.object({ type: z.literal("text") }).strict(),
  z
    .object({
      type: z.literal("json_schema"),
      json_schema: z
        .object({
          name: z.string().regex(/^[A-Za-z0-9_-]{1,64}$/),
          strict: z.literal(true),
          schema: canonicalJson,
        })
        .strict(),
    })
    .strict(),
]);
export type HostResponseFormat = z.infer<typeof hostResponseFormatSchema>;
export const hostPrepareResponseSchema = z
  .object({
    version: z.literal(1),
    contactRequestId: z.uuid(),
    workContextId: z.uuid(),
    roleScope: hostRoleScopeSchema,
    tokenBudget: hostTokenBudgetSchema,
    responseFormat: hostResponseFormatSchema.optional(),
    allowedToolNames: z.array(z.string().min(1).max(512)).max(1024),
    currentSources: z.array(canonicalJson).max(64).default([]),
    allowedHistoryUserKeys: z
      .array(z.string().min(1).max(4096))
      .max(2048)
      .default([]),
    usage: canonicalJson,
    contacts: z.array(
      z
        .object({ contactRef: z.string().min(1), observation: canonicalJson })
        .strict(),
    ),
    scope: hostScopeSchema,
  })
  .strict();
export const hostAdmittedRequestSchema = z
  .object({
    ...contactIdentity,
    projection: canonicalJson,
    occurrences: z.array(
      z
        .object({ contactRef: z.string().min(1), receipt: canonicalJson })
        .strict(),
    ),
  })
  .strict();
export const hostAcknowledgementSchema = z
  .object({ version: z.literal(1), accepted: z.literal(true) })
  .strict();
export const hostToolExecutionRequestSchema = z
  .object({
    ...contactIdentity,
    toolName: z.string().min(1).max(512),
    toolCallId: z.string().min(1).max(2048),
    phase: z.enum(["prepare", "execute"]),
  })
  .strict();
export const hostToolExecutionResponseSchema = z
  .object({
    version: z.literal(1),
    contactRequestId: z.uuid(),
    toolName: z.string().min(1).max(512),
    toolCallId: z.string().min(1).max(2048),
    allowed: z.literal(true),
    usage: canonicalJson,
  })
  .strict();
export type HostToolExecutionRequest = z.infer<
  typeof hostToolExecutionRequestSchema
>;
export type HostToolExecutionResponse = z.infer<
  typeof hostToolExecutionResponseSchema
>;
export const hostHandoffRequestSchema = z
  .object({
    version: z.literal(1),
    requestId: z.uuid(),
    subjectId: z.uuid(),
    parentRunId: z.string().min(1),
    parentSessionKey: z.string().min(1),
    parentSessionId: z.string().min(1),
    parentNousSessionId: z.uuid(),
    childNativeAgentId: z.string().min(1),
    childSessionKey: z.string().min(1),
    childSessionId: z.string().min(1),
    childNousSessionId: z.uuid(),
    childCurrentContextDigest: z
      .string()
      .regex(/^[a-f0-9]{64}$/)
      .optional(),
    contextMode: z.literal("isolated"),
  })
  .strict();
/** Native durable child causality; no model or ordinary tool input can mint this request. */
export const hostCompletionRequestSchema = z
  .object({
    version: z.literal(1),
    subjectId: z.uuid(),
    parentRunId: z.string().min(1).max(2048).optional(),
    parentSessionKey: z.string().min(1),
    parentSessionId: z.string().min(1).optional(),
    parentNousSessionId: z.uuid().optional(),
    childRunId: z.string().min(1).max(2048),
    childSessionKey: z.string().min(1),
    childSessionId: z.string().min(1),
    childNousSessionId: z.uuid(),
    outcome: z.enum(["completed", "failed", "cancelled"]),
    endedAt: z.iso.datetime(),
    recover: z.boolean().optional(),
  })
  .strict();
export const hostCompletionResponseSchema = z
  .object({
    version: z.literal(1),
    accepted: z.literal(true),
    activityRef: z.string().min(1),
    parentRunId: z.string().min(1),
    resultRetained: z.literal(true),
    resultIds: z.array(z.uuid()).max(64),
    resultsOmitted: z.boolean(),
  })
  .strict();
export type HostCompletionRequest = z.infer<typeof hostCompletionRequestSchema>;
export type HostCompletionResponse = z.infer<
  typeof hostCompletionResponseSchema
>;
/** Exact admitted contact terminal; native outer settlement owns this marker. */
export const hostRunTerminalRequestSchema = z
  .object({
    ...contactIdentity,
    outcome: z.enum(["completed", "failed", "cancelled"]),
    endedAt: z.iso.datetime(),
  })
  .strict();
export const hostRunTerminalResponseSchema = z
  .object({
    version: z.literal(1),
    accepted: z.literal(true),
    runId: z.string().min(1),
    activityRef: z.string().min(1),
    resultRetained: z.literal(true),
    ready: z.literal(true),
  })
  .strict();
export type HostRunTerminalRequest = z.infer<
  typeof hostRunTerminalRequestSchema
>;
export type HostRunTerminalResponse = z.infer<
  typeof hostRunTerminalResponseSchema
>;
/** Trusted Host release selector; it cannot supply replacement result text. */
export const hostResultReleaseRequestSchema = z
  .object({
    version: z.literal(1),
    requestId: z.uuid(),
    subjectId: z.uuid(),
    parentRunId: z.string().min(1),
    childRunId: z.string().min(1),
    resultIds: z.array(z.uuid()).min(1).max(64),
  })
  .strict();
export const hostResultReleaseResponseSchema = z
  .object({
    version: z.literal(1),
    accepted: z.literal(true),
    activityRef: z.string().min(1),
    contactNature: z.literal("subject_experience"),
    contacts: hostPrepareResponseSchema.shape.contacts,
    opportunity: z
      .object({
        kind: z.literal("activity_result"),
        parentRunId: z.string().min(1),
        childRunId: z.string().min(1),
        releaseRef: z.uuid(),
      })
      .strict(),
  })
  .strict();
export type HostResultReleaseRequest = z.infer<
  typeof hostResultReleaseRequestSchema
>;
export type HostResultReleaseResponse = z.infer<
  typeof hostResultReleaseResponseSchema
>;

export const hostFeedbackRequestSchema = z.discriminatedUnion("stage", [
  z
    .object({
      ...contactIdentity,
      stage: z.literal("pending"),
      observation: canonicalJson,
    })
    .strict(),
  z
    .object({
      ...contactIdentity,
      stage: z.literal("committed"),
      observationRequestId: z.uuid(),
      receipt: canonicalJson,
    })
    .strict(),
]);
// Pending retries return the first durably saved observation, including its
// actual acquisition timestamp. Hosts then own reconciliation until committed.
export const hostFeedbackResponseSchema = z.discriminatedUnion("stage", [
  z
    .object({
      version: z.literal(1),
      stage: z.literal("pending"),
      accepted: z.literal(true),
      retainedOnly: z.boolean().optional(),
      observation: canonicalJson,
    })
    .strict(),
  z
    .object({
      version: z.literal(1),
      stage: z.literal("committed"),
      accepted: z.literal(true),
    })
    .strict(),
]);
export type NativeIdentity = z.infer<typeof nativeIdentitySchema>;
export type HostPrepareRequest = z.infer<typeof hostPrepareRequestSchema>;
export type HostPrepareResponse = z.infer<typeof hostPrepareResponseSchema>;
export type HostAdmittedRequest = z.infer<typeof hostAdmittedRequestSchema>;
export type HostHandoffRequest = z.infer<typeof hostHandoffRequestSchema>;
export type HostFeedbackRequest = z.infer<typeof hostFeedbackRequestSchema>;
export type HostFeedbackResponse = z.infer<typeof hostFeedbackResponseSchema>;
/** HTTP already admits only JSON values. Keep canonical protobuf objects opaque
 * here; strict z.json/fromJson remain the runtime authorities. Recursive JSON
 * value definitions cannot survive arbitrary OpenAPI component embedding. */
function httpSchema(schema: z.ZodType) {
  const generated = z.toJSONSchema(schema, {
    target: "draft-07",
    override({ zodSchema, jsonSchema }) {
      if (zodSchema === canonicalJson) jsonSchema.additionalProperties = {};
    },
  });
  // Only discard unreachable generated definitions; every actual wire field is
  // still derived from the owning runtime schema, never copied or stripped.
  const { definitions, ...body } = generated;
  if (definitions && !JSON.stringify(body).includes('"$ref"'))
    delete generated.definitions;
  return generated;
}
export const HOST_BRIDGE_JSON_SCHEMAS = {
  resultReleaseRequest: httpSchema(hostResultReleaseRequestSchema),
  resultReleaseResponse: httpSchema(hostResultReleaseResponseSchema),
  runTerminalRequest: httpSchema(hostRunTerminalRequestSchema),
  runTerminalResponse: httpSchema(hostRunTerminalResponseSchema),
  completionRequest: httpSchema(hostCompletionRequestSchema),
  completionResponse: httpSchema(hostCompletionResponseSchema),
  toolExecutionRequest: httpSchema(hostToolExecutionRequestSchema),
  toolExecutionResponse: httpSchema(hostToolExecutionResponseSchema),
  prepareRequest: httpSchema(hostPrepareRequestSchema),
  prepareResponse: httpSchema(hostPrepareResponseSchema),
  admittedRequest: httpSchema(hostAdmittedRequestSchema),
  acknowledgement: httpSchema(hostAcknowledgementSchema),
  handoffRequest: httpSchema(hostHandoffRequestSchema),
  feedbackRequest: httpSchema(hostFeedbackRequestSchema),
  feedbackResponse: httpSchema(hostFeedbackResponseSchema),
};

// Canonical codecs are public so Hosts never import the bundled internal
// Protocol package or reproduce cognition message fields in application DTOs.
import {
  create,
  fromJson,
  toJson,
  type JsonObject,
  type JsonValue,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import {
  CognitionUsageSchema,
  ObservationInputSchema,
  AcceptedObservationSchema,
  ProjectionSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
export type {
  CognitionUsage,
  ObservationInput,
  AcceptedObservation,
  Projection,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
export { timestampFromDate } from "@bufbuild/protobuf/wkt";
export const encodeCognitionUsage = (
  value: MessageInitShape<typeof CognitionUsageSchema>,
): JsonObject =>
  toJson(
    CognitionUsageSchema,
    create(CognitionUsageSchema, value),
  ) as JsonObject;
export const decodeCognitionUsage = (json: JsonValue) =>
  fromJson(CognitionUsageSchema, json);
export const encodeObservationInput = (
  value: MessageInitShape<typeof ObservationInputSchema>,
): JsonObject =>
  toJson(
    ObservationInputSchema,
    create(ObservationInputSchema, value),
  ) as JsonObject;
export const decodeObservationInput = (json: JsonValue) =>
  fromJson(ObservationInputSchema, json);
export const encodeAcceptedObservation = (
  value: MessageInitShape<typeof AcceptedObservationSchema>,
): JsonObject =>
  toJson(
    AcceptedObservationSchema,
    create(AcceptedObservationSchema, value),
  ) as JsonObject;
export const decodeAcceptedObservation = (json: JsonValue) =>
  fromJson(AcceptedObservationSchema, json);
export const encodeProjection = (
  value: MessageInitShape<typeof ProjectionSchema>,
): JsonObject =>
  toJson(ProjectionSchema, create(ProjectionSchema, value)) as JsonObject;
export const decodeProjection = (json: JsonValue) =>
  fromJson(ProjectionSchema, json);

export const HOST_COGNITION_USAGE_CONTEXT_KEY = "cognition_usage";
export const HOST_CONTACT_CONTEXT_KEY = "contact_request_id";
/** Apply once when the Host persists the actual contact/pending fact. Replays
 * reuse that saved canonical ObservationInput, rather than a newer scope. */
export function withHostObservationScope(
  value: MessageInitShape<typeof ObservationInputSchema>,
  scope: {
    contactRequestId: string;
    activityRef: string;
    usage: MessageInitShape<typeof CognitionUsageSchema>;
  },
) {
  const observation = create(ObservationInputSchema, value);
  observation.context = {
    ...observation.context,
    [HOST_ACTIVITY_CONTEXT_KEY]: scope.activityRef,
    [HOST_CONTACT_CONTEXT_KEY]: scope.contactRequestId,
    [HOST_COGNITION_USAGE_CONTEXT_KEY]: encodeCognitionUsage(scope.usage),
  };
  return observation;
}

/** Host-selected contacts become readable only through their actual accepted roots.
 * The caller must supply receipts for exactly its selected contacts. */
export function withAcceptedContactRoots(
  value: MessageInitShape<typeof CognitionUsageSchema>,
  subjectId: string,
  receipts: readonly MessageInitShape<typeof AcceptedObservationSchema>[],
) {
  const usage = create(CognitionUsageSchema, value);
  if (
    !subjectId ||
    !usage.sourceRead ||
    usage.sourceRead.subjectId !== subjectId
  )
    throw new Error(
      "Accepted contacts require an explicit same-Subject SourceRead scope",
    );
  const seen = new Set(
    usage.sourceRead.sourceRefs.map((ref) => `${ref.kind}:${ref.value}`),
  );
  for (const receipt of receipts) {
    const id = create(AcceptedObservationSchema, receipt).occurrenceId;
    if (!id)
      throw new Error("Accepted contact requires its actual occurrenceId");
    const key = `occurrence:${id}`;
    if (!seen.has(key)) {
      usage.sourceRead.sourceRefs.push(
        create(CognitionUsageSchema, {
          sourceRead: {
            subjectId,
            sourceRefs: [{ kind: "occurrence", value: id }],
          },
        }).sourceRead!.sourceRefs[0]!,
      );
      seen.add(key);
    }
  }
  return usage;
}
