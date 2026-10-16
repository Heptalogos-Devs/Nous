// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, toJson } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  ObservationInputSchema,
  ProjectionSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { eventIdentity, outcomeObservation } from "@nous-wave/client/managed";
import {
  HOST_ACTIVITY_CONTEXT_KEY,
  HOST_COGNITION_USAGE_CONTEXT_KEY,
  decodeCognitionUsage,
  encodeProjection,
  encodeCognitionUsage,
  withHostObservationScope,
  hostPrepareRequestSchema,
  hostContactIdentitySchema,
  type HostPrepareRequest,
} from "@nous-wave/client/host-bridge";
import type { NousClient } from "@nous-wave/client";
import type { Context } from "@openclaw/ai";
import { z } from "zod";
import {
  HostBridge,
  decodeHostObservation,
  observationJson,
  occurrenceJson,
  type HostedPermission,
} from "./host.js";
import { runHostedTool } from "./readonly-tools.js";
import type {
  ManagedToolExecution,
  ManagedToolExecutionGuard,
  ManagedModelInput,
  ManagedModelApplication,
} from "./contract.js";
const markerSchema = z
  .object({
    identity: hostPrepareRequestSchema,
    observedAt: z.iso.datetime(),
    outcome: z.enum(["completed", "failed", "cancelled"]).optional(),
    projectionId: z.string().optional(),
  })
  .strict();
type Message = Context["messages"][number];
type MarkedMessage = Message & {
  nousHostBridge?: z.infer<typeof markerSchema>;
};

/** Ephemeral bookkeeping only. Actual events remain in the native transcript;
 * the Host owns durable pending observations and replay acknowledgements. */
export class HostedLifecycle {
  private readonly current = new Map<
    string,
    HostedPermission & { identity: HostPrepareRequest }
  >();
  private readonly acknowledged = new Set<string>();
  constructor(
    readonly bridge: HostBridge,
    private readonly subjectId: string,
    private readonly client: () => Promise<NousClient>,
    private readonly log: (message: string) => void,
    private readonly consumerRef: string,
  ) {}
  private key(input: ManagedModelInput) {
    return `${input.sessionId}:${input.runId}`;
  }
  async prepare(
    input: ManagedModelInput,
    nousSessionId: string,
    invocationId: string,
  ) {
    if (!input.sessionKey)
      throw new Error("Hosted input requires a persisted native Session key");
    const digest = new Uint8Array(
      await crypto.subtle.digest(
        "SHA-256",
        new TextEncoder().encode(input.prompt),
      ),
    );
    const identity: HostPrepareRequest = {
      version: 1,
      subjectId: this.subjectId,
      nousSessionId,
      invocationId,
      contactRequestId: await eventIdentity(`${invocationId}:contact`),
      native: {
        sessionKey: input.sessionKey,
        sessionId: input.sessionId,
        runId: input.runId,
        agentId: input.agentId,
        historyGeneration: input.historyGeneration,
        ...(input.parentSessionKey
          ? { parentSessionKey: input.parentSessionKey }
          : {}),
      },
      promptDigest: [...digest]
        .map((v) => v.toString(16).padStart(2, "0"))
        .join(""),
    };
    const permission = {
      ...(await this.bridge.prepare(identity, input.signal)),
      identity,
    };
    this.current.set(this.key(input), permission);
    return permission;
  }
  async authorizeToolExecution(
    input: ManagedModelInput,
    application: ManagedModelApplication,
    execution: ManagedToolExecution,
  ): Promise<ManagedToolExecutionGuard> {
    const permission = this.current.get(this.key(input));
    if (
      !permission ||
      permission.identity.invocationId !== application.invocationId ||
      !permission.response.allowedToolNames.includes(execution.toolName)
    )
      throw new Error("No current Host contact permits the actual native tool");
    const response = await this.bridge.toolExecution(
      {
        ...hostContactIdentitySchema.strip().parse(permission.identity),
        ...execution,
      },
      input.signal,
    );
    const usage = decodeCognitionUsage(response.usage);
    if (
      JSON.stringify(encodeCognitionUsage(usage)) !==
      JSON.stringify(encodeCognitionUsage(permission.usage))
    )
      throw new Error(
        "Host tool usage changed from the actual admitted contact",
      );
    input.signal.throwIfAborted();
    return {
      run: (implementation) =>
        runHostedTool(
          {
            identity: permission.identity,
            execution,
            usage,
            workContextId: permission.response.workContextId,
            client: this.client,
            signal: input.signal,
            consumerRef: this.consumerRef,
          },
          implementation,
        ),
    };
  }
  async admitted(
    permission: HostedPermission & { identity: HostPrepareRequest },
    projection: Parameters<typeof toJson<typeof ProjectionSchema>>[1],
    occurrences: {
      contactRef: string;
      receipt: ReturnType<typeof occurrenceJson>;
    }[],
    signal: AbortSignal,
  ) {
    await this.bridge.admitted(
      {
        ...hostContactIdentitySchema.strip().parse(permission.identity),
        projection: encodeProjection(projection),
        occurrences,
      },
      signal,
    );
  }
  private mark(input: ManagedModelInput, message: Message) {
    const marked = message as MarkedMessage;
    if (!marked.nousHostBridge) {
      const permission = this.current.get(this.key(input));
      if (!permission)
        throw new Error("Actual native event has no admitted Host context");
      marked.nousHostBridge = {
        identity: permission.identity,
        observedAt: new Date().toISOString(),
      };
    }
    return markerSchema.parse(marked.nousHostBridge);
  }
  private async submit(
    marker: z.infer<typeof markerSchema>,
    observation: NonNullable<Parameters<typeof observationJson>[0]>,
    signal?: AbortSignal,
  ) {
    if (this.acknowledged.has(observation.requestId!)) return;
    const originalScope = [...this.current.values()].find(
      (value) =>
        value.identity.contactRequestId === marker.identity.contactRequestId,
    );
    if (originalScope)
      observation = withHostObservationScope(observation, {
        contactRequestId: marker.identity.contactRequestId,
        activityRef: originalScope.response.scope.activityRef,
        usage: originalScope.usage,
      });
    const pending = await this.bridge.feedback(
      {
        ...hostContactIdentitySchema.strip().parse(marker.identity),
        stage: "pending",
        observation: observationJson(observation),
      },
      signal,
    );
    if (pending.stage !== "pending")
      throw new Error("Host did not persist pending observation");
    const saved = decodeHostObservation(pending.observation, marker.identity);
    const actualUsage = decodeCognitionUsage(
      saved.context![HOST_COGNITION_USAGE_CONTEXT_KEY]!,
    );
    if (
      actualUsage.agentInstanceId !==
      `${marker.identity.native.sessionId}:${marker.identity.native.runId}`
    )
      throw new Error(
        "Host feedback changed the original native cognition scope",
      );
    if (
      saved.requestId !== observation.requestId ||
      typeof saved.context?.[HOST_ACTIVITY_CONTEXT_KEY] !== "string" ||
      !saved.context[HOST_ACTIVITY_CONTEXT_KEY]
    )
      throw new Error(
        "Host pending observation lost its native event or Activity identity",
      );
    const receipt = await (
      await this.client()
    ).runtime.recordObservation(saved, { signal });
    const ack = await this.bridge.feedback(
      {
        ...hostContactIdentitySchema.strip().parse(marker.identity),
        stage: "committed",
        observationRequestId: saved.requestId,
        receipt: occurrenceJson(receipt),
      },
      signal,
    );
    if (ack.stage !== "committed")
      throw new Error("Host did not acknowledge committed observation");
    this.acknowledged.add(saved.requestId!);
  }
  private async replay(
    message: Message,
    marker: z.infer<typeof markerSchema>,
    signal?: AbortSignal,
  ) {
    if (marker.identity.subjectId !== this.subjectId)
      throw new Error("Native Host contact has a foreign Subject");
    const requestId = await eventIdentity(
      `${marker.identity.native.sessionId}:${marker.identity.native.historyGeneration}:${message.role}:${message.timestamp}:${"toolCallId" in message ? message.toolCallId : JSON.stringify(message.content)}`,
    );
    const permission = [...this.current.values()].find(
      (value) =>
        value.identity.contactRequestId === marker.identity.contactRequestId,
    );
    const observation = create(ObservationInputSchema, {
      subjectId: this.subjectId,
      sessionId: marker.identity.nousSessionId,
      requestId,
      sourceClass: message.role === "toolResult" ? "tool" : "generated",
      externalObjectRef: `object:openclaw:${requestId}`,
      conversationRef: marker.identity.native.sessionKey,
      observedAt: timestampFromDate(new Date(marker.observedAt)),
      occurredTime: {
        value: {
          case: "instant",
          value: timestampFromDate(new Date(message.timestamp)),
        },
      },
      material: {
        case: "inlineText",
        value: {
          mediaType: "application/json",
          text: JSON.stringify(
            message.role === "toolResult"
              ? {
                  content: message.content,
                  details: message.details ?? null,
                  isError: message.isError,
                }
              : { content: message.content, delivery: "model_output" },
          ),
        },
      },
      admit: message.role === "toolResult",
      actorEntityRef: permission?.response.scope.actorEntityRef,
      context: {
        ...permission?.response.scope.context,
        ...(permission
          ? {
              [HOST_ACTIVITY_CONTEXT_KEY]:
                permission.response.scope.activityRef,
            }
          : {}),
        native_session_id: marker.identity.native.sessionId,
        native_run_id: marker.identity.native.runId,
        contact_request_id: marker.identity.contactRequestId,
        role: message.role,
        ...(message.role === "toolResult"
          ? {
              tool_name: message.toolName,
              tool_call_id: message.toolCallId,
              is_error: message.isError,
            }
          : { delivery: "model_output" }),
      },
    });
    await this.submit(marker, observation, signal);
    if (marker.outcome && marker.projectionId)
      await this.submit(
        marker,
        await outcomeObservation({
          subjectId: this.subjectId,
          sessionId: marker.identity.nousSessionId,
          observedAt: new Date(marker.observedAt),
          result: {
            invocationId: marker.identity.invocationId,
            applied: { projectionId: marker.projectionId, sourceRefs: [] },
            occurredAt: new Date(message.timestamp),
            outcome: marker.outcome,
          },
        }),
        signal,
      );
  }
  capture(
    input: ManagedModelInput,
    message: Message,
    application?: ManagedModelApplication,
    outcome?: "completed" | "failed" | "cancelled",
  ) {
    if (message.role === "user") return;
    const marker = this.mark(input, message);
    if (application && outcome) {
      marker.outcome = outcome;
      marker.projectionId = application.applied.projectionId;
    }
    (message as MarkedMessage).nousHostBridge = marker;
  }
  async observe(input: ManagedModelInput, message: Message) {
    if (message.role === "user") return;
    const marker = this.mark(input, message);
    try {
      await this.replay(message, marker);
    } catch (error) {
      this.log(
        `Host feedback pending; actual native result retained: ${error instanceof Error ? error.message : "unavailable"}`,
      );
    }
  }
  async report(
    input: ManagedModelInput,
    application: ManagedModelApplication,
    outcome: "completed" | "failed" | "cancelled",
    message?: Message,
  ) {
    if (!message) {
      const permission = this.current.get(this.key(input));
      if (!permission) return;
      const marker = {
        identity: permission.identity,
        observedAt: new Date().toISOString(),
      };
      try {
        await this.submit(
          marker,
          await outcomeObservation({
            subjectId: this.subjectId,
            sessionId: permission.identity.nousSessionId,
            observedAt: new Date(marker.observedAt),
            result: {
              invocationId: application.invocationId,
              applied: application.applied,
              outcome,
              occurredAt: new Date(),
            },
          }),
        );
      } catch (error) {
        this.log(
          `Host outcome feedback unavailable: ${error instanceof Error ? error.message : "unavailable"}`,
        );
      }
      return;
    }
    const marker = this.mark(input, message);
    marker.outcome = outcome;
    marker.projectionId = application.applied.projectionId;
    (message as MarkedMessage).nousHostBridge = marker;
    await this.observe(input, message);
  }
  async flush(messages: readonly unknown[], signal?: AbortSignal) {
    for (const value of messages) {
      if (!value || typeof value !== "object") continue;
      const message = value as MarkedMessage;
      if (
        (message.role !== "assistant" && message.role !== "toolResult") ||
        !message.nousHostBridge
      )
        continue;
      await this.replay(
        message,
        markerSchema.parse(message.nousHostBridge),
        signal,
      );
    }
  }
}
