// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  ManagedContextRequestSchema,
  EvidenceRefSchema,
  ObservationInputSchema,
  type CognitionUsage,
  type ContextCursor,
  type ContextSegment,
  type DomainStamp,
  type EpistemicView,
  type CognitiveRef,
  type ManagedContextResponse,
  type Projection,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { NousClient, RequestOptions } from "./index.js";
import type { HostRoleScope } from "./host-bridge.js";

export type { CognitionUsage, ContextSegment, DomainStamp, EpistemicView };
export interface AppliedCognition {
  readonly projectionId: string;
  readonly sourceRefs: readonly CognitiveRef[];
}
export interface ContextRenderer<T> {
  /** Receives clean host input; the renderer never mutates execution history. */
  render(
    base: T,
    segments: readonly ContextSegment[],
    usage: CognitionUsage,
    clock: DomainStamp,
    epistemicView?: EpistemicView,
  ): T;
}
export interface ActualInvocationOutcome {
  invocationId: string;
  applied: AppliedCognition;
  occurredAt: Date;
  outcome: "completed" | "failed" | "cancelled";
  observations?: readonly MessageInitShape<typeof ObservationInputSchema>[];
}

/** Public contract for one serial worker's managed cognition lifecycle. */
export interface ManagedBinding {
  sessionId: string;
  prepare(input: {
    usage: CognitionUsage;
    workContextId?: string;
    cognitionTokenBudget: number;
    currentMaterials?: readonly MessageInitShape<typeof EvidenceRefSchema>[];
    signal: AbortSignal;
  }): Promise<ManagedContextResponse>;
  apply<T>(
    base: T,
    response: ManagedContextResponse,
    renderer: ContextRenderer<T>,
  ): { input: T; applied: AppliedCognition };
  /** Exact full Projection used by apply, including APPEND's retained prefix. */
  appliedProjection(applied: AppliedCognition): Projection;
  admit(
    result: Pick<
      ActualInvocationOutcome,
      "invocationId" | "applied" | "occurredAt"
    >,
    request?: RequestOptions,
  ): Promise<void>;
  report(
    result: ActualInvocationOutcome,
    request?: RequestOptions,
  ): Promise<void>;
  close(): Promise<void>;
}

/** One serial worker's ephemeral cursor/slice, backed only by public Nous operations. */
export function createManagedBinding(options: {
  client: () => NousClient | Promise<NousClient>;
  subjectId: string;
  sessionId: string;
  consumerId: string;
  /** Trusted integration caller scope. Standalone defaults to Subject experience. */
  roleScope?: HostRoleScope;
  /** Execution-only outcomes remain with their RoleRun owner, outside cognition. */
  retainExecutionOutcome?: (result: ActualInvocationOutcome) => Promise<void>;
}): ManagedBinding {
  let cursor: ContextCursor | undefined;
  let current: Projection | undefined;
  let pending:
    { response: ManagedContextResponse; signal: AbortSignal } | undefined;
  let preparing = false;
  let closed = false;
  const applications = new WeakMap<AppliedCognition, Projection>();
  const admissions = new WeakMap<AppliedCognition, Set<string>>();
  const assertOpen = () => {
    if (closed) throw new Error("Managed cognition binding is closed");
  };
  return {
    sessionId: options.sessionId,
    async prepare(input: {
      usage: CognitionUsage;
      workContextId?: string;
      cognitionTokenBudget: number;
      currentMaterials?: readonly MessageInitShape<typeof EvidenceRefSchema>[];
      signal: AbortSignal;
    }): Promise<ManagedContextResponse> {
      assertOpen();
      input.signal.throwIfAborted();
      if (preparing)
        throw new Error("Parallel workers require separate managed bindings");
      if (
        !Number.isSafeInteger(input.cognitionTokenBudget) ||
        input.cognitionTokenBudget < 512
      )
        throw new Error("Insufficient cognition input budget");
      preparing = true;
      pending = undefined;
      try {
        const client = await options.client();
        const response = await client.cognition.buildManagedContext(
          create(ManagedContextRequestSchema, {
            projection: {
              subjectId: options.subjectId,
              sessionId: options.sessionId,
              consumerId: options.consumerId,
              activeWorkContextId: input.workContextId,
              usage: options.roleScope
                ? {
                    ...input.usage,
                    role: options.roleScope.role,
                    purpose: options.roleScope.purpose,
                    view: options.roleScope.view,
                    contactNature: options.roleScope.contactNature,
                  }
                : input.usage,
              currentMaterials: [...(input.currentMaterials ?? [])],
              // UTF-8 byte count bounds text tokens conservatively. The host
              // measures the complete rendered input, including attribution.
              maxTextBytes: input.cognitionTokenBudget - 256,
            },
            knownCursor: cursor,
          }),
          { signal: input.signal },
        );
        assertOpen();
        input.signal.throwIfAborted();
        pending = { response, signal: input.signal };
        return response;
      } finally {
        preparing = false;
      }
    },
    apply<T>(
      base: T,
      response: ManagedContextResponse,
      renderer: ContextRenderer<T>,
    ) {
      assertOpen();
      if (pending?.response !== response)
        throw new Error(
          "Context response is not this binding's pending preparation",
        );
      pending.signal.throwIfAborted();
      const projection = response.projection;
      const nextCursor = response.cursor;
      if (
        !projection?.usage ||
        !nextCursor ||
        projection.subjectId !== options.subjectId ||
        projection.sessionId !== options.sessionId ||
        projection.consumerId !== options.consumerId
      )
        throw new Error(
          "Required cognition preparation is unavailable or has foreign scope",
        );
      if (
        !projection.semanticClock?.at ||
        !projection.semanticClock.recordedAt ||
        !projection.semanticClock.timeDomainRef.trim()
      )
        throw new Error(
          "Required Subject semantic clock is missing; update and prepare the official Nous runtime and plugin",
        );
      let segments: ContextSegment[];
      if (response.kind === "RESET") segments = projection.segments;
      else if (
        response.kind === "APPEND" &&
        current &&
        cursor &&
        nextCursor.trackId === cursor.trackId &&
        nextCursor.epochId === cursor.epochId &&
        nextCursor.revision ===
          cursor.revision + (projection.segments.length ? 1 : 0)
      )
        segments = [...current.segments, ...projection.segments];
      else
        throw new Error(
          "Managed context delta does not extend the applied cursor",
        );
      const next = createProjection(projection, segments);
      const input = renderer.render(
        base,
        structuredClone(segments),
        structuredClone(projection.usage),
        structuredClone(projection.semanticClock),
        structuredClone(projection.epistemicView),
      );
      pending.signal.throwIfAborted();
      const sourceRefs = [
        ...new Map(
          segments
            .flatMap((segment) => segment.sourceRefs)
            .map((ref) => [`${ref.kind}:${ref.value}`, ref]),
        ).values(),
      ];
      const applied: AppliedCognition = Object.freeze({
        projectionId: projection.projectionId,
        sourceRefs: Object.freeze(structuredClone(sourceRefs)),
      });
      applications.set(applied, next);
      current = next;
      cursor = structuredClone(nextCursor);
      pending = undefined;
      return { input, applied };
    },
    appliedProjection(applied) {
      assertOpen();
      const projection = applications.get(applied);
      if (!projection)
        throw new Error(
          "Projection requires an actual application from this binding",
        );
      return structuredClone(projection);
    },
    async admit(
      result: Pick<
        ActualInvocationOutcome,
        "invocationId" | "applied" | "occurredAt"
      >,
      request?: RequestOptions,
    ): Promise<void> {
      assertOpen();
      const projection = applications.get(result.applied);
      if (!projection || !result.invocationId.trim())
        throw new Error(
          "Model input admission requires an actual application from this binding",
        );
      const client = await options.client();
      const materialKinds = new Set([
        "occurrence",
        "derived_representation",
        "derived_region",
      ]);
      const references = [
        ...new Map(
          projection.segments
            .flatMap((segment) => [
              ...segment.sourceRefs,
              ...segment.evidence
                .filter(
                  (e) => e.basisRole === "actual_occurrence" && e.reference,
                )
                .map((e) => e.reference!),
            ])
            .filter(
              (ref) =>
                ref.kind.endsWith("_revision") || materialKinds.has(ref.kind),
            )
            .map((ref) => [`${ref.kind}:${ref.value}`, ref]),
        ).values(),
      ];
      for (
        let offset = 0;
        offset < Math.max(references.length, 1);
        offset += 256
      ) {
        const admitted = await client.runtime.reportUse(
          {
            subjectId: options.subjectId,
            sessionId: options.sessionId,
            consumerRef: options.consumerId,
            modelInput: true,
            events: await Promise.all(
              references.slice(offset, offset + 256).map(async (reference) => {
                const source = projection.segments.find((segment) =>
                  segment.sourceRefs.some(
                    (ref) =>
                      ref.kind === reference.kind &&
                      ref.value === reference.value,
                  ),
                );
                const actual = source?.evidence.find(
                  (e) =>
                    e.basisRole === "actual_occurrence" &&
                    e.reference?.kind === "occurrence",
                )?.reference?.value;
                return {
                  eventId: await eventIdentity(
                    `${result.invocationId}:presented:${reference.kind}:${reference.value}`,
                  ),
                  reference,
                  kind: "presented",
                  occurredAt: timestampFromDate(result.occurredAt),
                  context: {
                    invocation_id: result.invocationId,
                    projection_id: projection.projectionId,
                    stage: "model_input",
                    ...(actual ? { actual_occurrence_id: actual } : {}),
                  },
                };
              }),
            ),
          },
          request,
        );
        if (!admitted.modelInputChecked)
          throw new Error(
            "Core did not enforce the required model input history check",
          );
      }
      let accepted = admissions.get(result.applied);
      if (!accepted) admissions.set(result.applied, (accepted = new Set()));
      accepted.add(result.invocationId);
    },
    async report(
      result: ActualInvocationOutcome,
      request?: RequestOptions,
    ): Promise<void> {
      assertOpen();
      const projection = applications.get(result.applied);
      const client = await options.client();
      if (!projection || !result.invocationId.trim())
        throw new Error(
          "Outcome requires an actual application from this binding",
        );
      if (!admissions.get(result.applied)?.has(result.invocationId))
        throw new Error("Outcome requires an admitted native model input");
      if (options.roleScope?.contactNature === "execution_only") {
        if (!options.retainExecutionOutcome)
          throw new Error(
            "Execution-only managed binding requires its actual RoleRun result owner",
          );
        await options.retainExecutionOutcome(result);
        return;
      }
      await client.runtime.recordObservation(
        await outcomeObservation({
          subjectId: options.subjectId,
          sessionId: options.sessionId,
          result,
        }),
        request,
      );
      for (const observation of result.observations ?? []) {
        if (
          observation.subjectId !== options.subjectId ||
          observation.sessionId !== options.sessionId
        )
          throw new Error(
            "Actual observation has foreign Subject or Session scope",
          );
        await client.runtime.recordObservation(observation, request);
      }
    },
    async close(): Promise<void> {
      closed = true;
      cursor = undefined;
      current = undefined;
      pending = undefined;
    },
  };
}

function createProjection(
  projection: Projection,
  segments: ContextSegment[],
): Projection {
  return structuredClone({ ...projection, segments });
}

/** Stable UUID for retrying the same actual native event, without durable SDK state. */
export async function eventIdentity(key: string): Promise<string> {
  const bytes = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key)),
  );
  bytes[6] = (bytes[6]! & 15) | 0x50;
  bytes[8] = (bytes[8]! & 63) | 0x80;
  const hex = [...bytes.slice(0, 16)]
    .map((value) => value.toString(16).padStart(2, "0"))
    .join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Canonical, replayable actual model outcome; Hosts can persist before submission. */
export async function outcomeObservation(input: {
  subjectId: string;
  sessionId: string;
  result: ActualInvocationOutcome;
  observedAt?: Date;
}): Promise<MessageInitShape<typeof ObservationInputSchema>> {
  const { result } = input;
  return {
    subjectId: input.subjectId,
    sessionId: input.sessionId,
    requestId: await eventIdentity(`${result.invocationId}:outcome`),
    sourceClass: "host_event",
    observedAt: input.observedAt
      ? timestampFromDate(input.observedAt)
      : undefined,
    occurredTime: {
      timeDomainRef: "time:real",
      extent: {
        value: { case: "instant", value: timestampFromDate(result.occurredAt) },
      },
    },
    material: {
      case: "inlineText",
      value: {
        mediaType: "application/json",
        text: JSON.stringify({
          invocation_id: result.invocationId,
          projection_id: result.applied.projectionId,
          outcome: result.outcome,
        }),
      },
    },
    admit: false,
    context: {
      invocation_id: result.invocationId,
      projection_id: result.applied.projectionId,
      outcome: result.outcome,
    },
  };
}

export { readOnlyQuery, readOnlySources } from "./readonly.js";
