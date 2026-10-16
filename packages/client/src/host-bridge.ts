// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Public, Node-free wire authority. Canonical protobuf JSON is decoded by its
// generated descriptor at the consuming boundary, never a second cognition DTO.
import { z } from "zod";
export const HOST_ACTIVITY_CONTEXT_KEY = "activity_ref";
export const HOST_BRIDGE_PATHS = Object.freeze({
  prepare: "/nous/v1/prepare",
  admitted: "/nous/v1/admitted",
  handoff: "/nous/v1/handoff",
  feedback: "/nous/v1/feedback",
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
export const hostPrepareRequestSchema = z
  .object({
    ...contactIdentity,
    promptDigest: z.string().regex(/^[a-f0-9]{64}$/),
  })
  .strict();
export const hostScopeSchema = z
  .object({
    activityRef: z.string().min(1),
    actorEntityRef: z.string().min(1).optional(),
    context: canonicalJson.default({}),
  })
  .strict();
export const hostPrepareResponseSchema = z
  .object({
    version: z.literal(1),
    contactRequestId: z.uuid(),
    workContextId: z.uuid(),
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
export const hostHandoffRequestSchema = z
  .object({
    version: z.literal(1),
    requestId: z.uuid(),
    subjectId: z.uuid(),
    parentSessionKey: z.string().min(1),
    parentSessionId: z.string().min(1),
    parentNousSessionId: z.uuid(),
    childSessionKey: z.string().min(1),
    childSessionId: z.string().min(1),
    childNousSessionId: z.uuid(),
    contextMode: z.literal("isolated"),
  })
  .strict();
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
export const HOST_BRIDGE_JSON_SCHEMAS = {
  prepareRequest: z.toJSONSchema(hostPrepareRequestSchema),
  prepareResponse: z.toJSONSchema(hostPrepareResponseSchema),
  admittedRequest: z.toJSONSchema(hostAdmittedRequestSchema),
  acknowledgement: z.toJSONSchema(hostAcknowledgementSchema),
  handoffRequest: z.toJSONSchema(hostHandoffRequestSchema),
  feedbackRequest: z.toJSONSchema(hostFeedbackRequestSchema),
  feedbackResponse: z.toJSONSchema(hostFeedbackResponseSchema),
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
