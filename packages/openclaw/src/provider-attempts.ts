// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  loadTranscriptEventsSync,
  listSessionEntries,
  resolveStorePath,
} from "openclaw/plugin-sdk/session-store-runtime";
import {
  nativeProviderAttemptsRequestSchema,
  nativeProviderAttemptsResponseSchema,
  NATIVE_PROVIDER_ATTEMPTS_METHOD,
} from "@nous-wave/client/host-bridge";
import type { OpenClawPluginApi } from "openclaw/plugin-sdk/core";
import { z } from "zod";
const json = z.record(z.string(), z.json());
const transcriptEntry = z
  .object({
    type: z.string(),
    customType: z.string().optional(),
    data: z.unknown().optional(),
    message: z
      .object({ role: z.string(), timestamp: z.number() })
      .passthrough()
      .optional(),
  })
  .passthrough();
const nativeRecord = z
  .object({
    version: z.literal(1),
    attemptId: z.string(),
    sequence: z.number().int().positive(),
    subjectId: z.string(),
    invocationId: z.string(),
    contactRequestId: z.string().optional(),
    native: z
      .object({
        runId: z.string(),
        sessionId: z.string(),
        sessionKey: z.string(),
        agentId: z.string(),
      })
      .passthrough(),
    stage: z.enum(["request", "response", "terminal"]),
    recordedAt: z.string(),
    provider: z.string().optional(),
    model: z.string().optional(),
    api: z.string().optional(),
    requestDigest: z.string().optional(),
    input: json.optional(),
    outcome: z.enum(["completed", "failed", "uncertain"]).optional(),
    response: json.optional(),
    usage: json.optional(),
    error: json.optional(),
    responseId: z.string().optional(),
  })
  .passthrough();
/** Uses the native transcript owner and Gateway authentication; no model work is admitted. */
export function registerProviderAttemptRead(
  api: OpenClawPluginApi,
  subjectId: string,
) {
  api.registerGatewayMethod(
    NATIVE_PROVIDER_ATTEMPTS_METHOD,
    ({ params, client, respond }) => {
      try {
        if (!client?.connect?.scopes?.includes("operator.admin"))
          throw Error("Operator detail authority is required");
        const input = nativeProviderAttemptsRequestSchema.parse(params);
        if (input.subjectId !== subjectId)
          throw Error("Native attempt names another Subject");
        const storePath = resolveStorePath(api.config?.session?.store, {
          agentId: input.agentId,
        });
        if (
          !listSessionEntries({
            agentId: input.agentId,
            storePath,
            readOnly: true,
          }).some((entry) => entry.sessionKey === input.sessionKey)
        )
          throw Error("The exact native session is unavailable");
        const events = loadTranscriptEventsSync({
          storePath,
          agentId: input.agentId,
          sessionId: input.sessionId,
          sessionKey: input.sessionKey,
        }).flatMap((value) => {
          const parsed = transcriptEntry.safeParse(value);
          return parsed.success ? [parsed.data] : [];
        });
        const records = events.flatMap((event) => {
          if (
            event.type !== "custom" ||
            event.customType !== "nous_provider_attempt"
          )
            return [];
          const parsed = nativeRecord.safeParse(event.data);
          if (!parsed.success) return [];
          const record = parsed.data;
          return record.subjectId === subjectId &&
            record.native.runId === input.runId &&
            record.native.sessionId === input.sessionId &&
            record.native.sessionKey === input.sessionKey &&
            record.native.agentId === input.agentId
            ? [record]
            : [];
        });
        const contacts = events.flatMap((event) => {
          if (
            event.type !== "custom" ||
            event.customType !== "nous_admitted_contact"
          )
            return [];
          const value = z
            .object({
              identity: z
                .object({
                  subjectId: z.string(),
                  invocationId: z.string(),
                  native: z
                    .object({
                      runId: z.string(),
                      sessionId: z.string(),
                      sessionKey: z.string(),
                      agentId: z.string(),
                    })
                    .passthrough(),
                })
                .passthrough(),
            })
            .safeParse(event.data);
          const identity = value.success ? value.data.identity : undefined;
          return identity?.subjectId === subjectId &&
            identity.native.runId === input.runId &&
            identity.native.sessionId === input.sessionId &&
            identity.native.sessionKey === input.sessionKey &&
            identity.native.agentId === input.agentId
            ? [identity]
            : [];
        });
        if (!records.length && !contacts.length)
          throw Error("No retained native identity matches this exact run");
        const requests = records
          .filter(
            (record) =>
              record.stage === "request" &&
              record.sequence > input.afterSequence,
          )
          .sort((a, b) => a.sequence - b.sequence);
        const attempts = requests.slice(0, input.limit).map((request) => {
          const response = records.find(
            (record) =>
              record.attemptId === request.attemptId &&
              record.stage === "response",
          );
          const terminal = records.find(
            (record) =>
              record.attemptId === request.attemptId &&
              record.stage === "terminal",
          );
          return {
            attemptId: request.attemptId,
            sequence: request.sequence,
            runId: input.runId,
            sessionKey: input.sessionKey,
            invocationId: request.invocationId,
            contactRequestId: request.contactRequestId,
            provider: request.provider,
            model: request.model,
            api: request.api,
            recordedAt: request.recordedAt,
            requestDigest: request.requestDigest,
            outcome: terminal?.outcome ?? "uncertain",
            response: response?.response,
            usage: terminal?.usage,
            costKnown: false,
            error: terminal?.error,
            responseId: terminal?.responseId,
            ...(input.includeInputs ? { input: request.input } : {}),
          };
        });
        const invocations = new Set(
          contacts.map((contact) => contact.invocationId),
        );
        const results = events.flatMap((event) => {
          if (event.type !== "message" || event.message?.role !== "assistant")
            return [];
          const message = event.message;
          const marker = z
            .object({
              identity: z.object({ invocationId: z.string() }).passthrough(),
            })
            .safeParse(Reflect.get(message, "nousHostBridge"));
          if (
            !marker.success ||
            !invocations.has(marker.data.identity.invocationId)
          )
            return [];
          return [
            {
              invocationId: marker.data.identity.invocationId,
              timestamp: message.timestamp,
              provider: message.provider,
              model: message.model,
              api: message.api,
              stopReason: message.stopReason,
              usage: message.usage,
            },
          ];
        });
        respond(
          true,
          nativeProviderAttemptsResponseSchema.parse({
            version: 1,
            subjectId,
            runId: input.runId,
            coverage: records.length ? "recorded" : "not_recorded",
            attempts,
            nextSequence:
              requests.length > input.limit ? attempts.at(-1)!.sequence : null,
            retainedResults: results.slice(-64),
            retainedResultsOmitted: results.length > 64,
          }),
        );
      } catch {
        respond(false, undefined, {
          code: "INVALID_REQUEST",
          message: "Exact authorized native provider records are unavailable",
        });
      }
    },
  );
}
