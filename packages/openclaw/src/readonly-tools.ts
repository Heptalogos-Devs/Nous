// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { AsyncLocalStorage } from "node:async_hooks";
import { Type } from "typebox";
import { z } from "zod";
import type { AnyAgentTool } from "openclaw/plugin-sdk/core";
import type { NousClient } from "@nous-wave/client";
import {
  eventIdentity,
  readOnlyQuery,
  readOnlySources,
} from "@nous-wave/client/managed";
import {
  timestampFromDate,
  type CognitionUsage,
  type HostPrepareRequest,
} from "@nous-wave/client/host-bridge";
import type { ManagedToolExecution } from "./contract.js";
interface TrustedToolScope {
  identity: HostPrepareRequest;
  execution: ManagedToolExecution;
  usage: CognitionUsage;
  workContextId: string;
  client: () => Promise<NousClient>;
  signal: AbortSignal;
  consumerRef: string;
}
// The native final execution guard installs this scope. There is no input DTO,
// MCP _meta, prompt or model-argument route for constructing it.
const trustedExecution = new AsyncLocalStorage<TrustedToolScope>();
export async function runHostedTool<T>(
  scope: TrustedToolScope,
  implementation: () => T | Promise<T>,
): Promise<T> {
  return await trustedExecution.run(scope, implementation);
}
function current(name: string, toolCallId: string) {
  const scope = trustedExecution.getStore();
  if (
    !scope ||
    scope.execution.phase !== "execute" ||
    scope.execution.toolName !== name ||
    scope.execution.toolCallId !== toolCallId
  )
    throw new Error(
      "Readonly cognition requires the current trusted native execution guard",
    );
  scope.signal.throwIfAborted();
  return scope;
}
async function presented(
  scope: TrustedToolScope,
  references: readonly { kind: string; value: string }[],
  queryId?: string,
) {
  const refs = [
    ...new Map(
      references.map((ref) => [`${ref.kind}:${ref.value}`, ref]),
    ).values(),
  ];
  const at = timestampFromDate(new Date());
  for (let offset = 0; offset < Math.max(refs.length, 1); offset += 256) {
    const batch = refs.slice(offset, offset + 256);
    const response = await (
      await scope.client()
    ).runtime.reportUse(
      {
        subjectId: scope.identity.subjectId,
        sessionId: scope.identity.nousSessionId,
        consumerRef: scope.consumerRef,
        modelInput: true,
        events: await Promise.all(
          batch.map(async (reference) => ({
            eventId: await eventIdentity(
              `${scope.identity.contactRequestId}:${scope.execution.toolCallId}:presented:${reference.kind}:${reference.value}`,
            ),
            reference,
            kind: "presented",
            occurredAt: at,
            ...(queryId ? { queryId } : {}),
            context: {
              invocation_id: scope.identity.invocationId,
              contact_request_id: scope.identity.contactRequestId,
              tool_name: scope.execution.toolName,
              tool_call_id: scope.execution.toolCallId,
              stage: "tool_history",
            },
          })),
        ),
      },
      { signal: scope.signal },
    );
    if (
      !response.modelInputChecked ||
      response.acceptedCount + response.duplicateCount !== batch.length
    )
      throw new Error(
        "Actual readonly source dependencies were not admitted to native history",
      );
  }
}
const queryInput = z.object({ query: z.string().min(1).max(32768) }).strict();
const readInput = z
  .object({
    references: z
      .array(
        z
          .object({ kind: z.string().min(1), value: z.string().min(1) })
          .strict(),
      )
      .min(1)
      .max(128),
    maxTextBytes: z.number().int().min(1).max(1048576).optional(),
  })
  .strict();
function result(value: unknown) {
  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(value, (_key, item: unknown) =>
          typeof item === "bigint" ? item.toString() : item,
        ),
      },
    ],
    details: {},
  };
}
/** Hosted tools obtain Subject, Session and SourceRead only from native TLS. */
export function hostedReadonlyTools(): AnyAgentTool[] {
  return [
    {
      name: "nous_query",
      label: "Read current cognition",
      description:
        "Query NousQL within the current Activity's actual Host-authorized sources. Returned references are exact. This tool cannot grant sources or mutate cognition.",
      parameters: Type.Object(
        { query: Type.String({ minLength: 1, maxLength: 32768 }) },
        { additionalProperties: false },
      ),
      async execute(toolCallId, args) {
        const scope = current("nous_query", toolCallId);
        const input = queryInput.parse(args);
        const response = await readOnlyQuery(
          await scope.client(),
          {
            subjectId: scope.identity.subjectId,
            sessionId: scope.identity.nousSessionId,
            workContextId: scope.workContextId,
            usage: scope.usage,
            nousql: input.query,
          },
          { signal: scope.signal },
        );
        await presented(
          scope,
          response.hits.flatMap((hit) => hit.revision ?? hit.reference ?? []),
          response.queryId,
        );
        if (response.hits.some((hit) => !hit.revision && !hit.reference))
          throw new Error("Readonly result has no actual source reference");
        return result({
          queryId: response.queryId,
          status: response.status,
          results: response.hits.map((hit) => ({
            reference: hit.revision ?? hit.reference,
            text: hit.text,
            authority: hit.authority,
            evidence: hit.evidence,
          })),
          degradation: [
            ...response.degradation,
            ...(response.resourceRecords.length
              ? [
                  {
                    code: "readonly_external_records_omitted",
                    detail:
                      "External resource bodies require their own actual source encounter",
                  },
                ]
              : []),
          ],
        });
      },
    },
    {
      name: "nous_read",
      label: "Read exact cognition sources",
      description:
        "Read bounded complete text/evidence for exact references under this native invocation's Host SourceRead. No Subject, scope or management authority can be supplied.",
      parameters: Type.Object(
        {
          references: Type.Array(
            Type.Object(
              {
                kind: Type.String({ minLength: 1 }),
                value: Type.String({ minLength: 1 }),
              },
              { additionalProperties: false },
            ),
            { minItems: 1, maxItems: 128 },
          ),
          maxTextBytes: Type.Optional(
            Type.Integer({ minimum: 1, maximum: 1048576 }),
          ),
        },
        { additionalProperties: false },
      ),
      async execute(toolCallId, args) {
        const scope = current("nous_read", toolCallId);
        const input = readInput.parse(args);
        const response = await readOnlySources(
          await scope.client(),
          {
            subjectId: scope.identity.subjectId,
            workContextId: scope.workContextId,
            usage: scope.usage,
            references: input.references,
            maxTextBytes: input.maxTextBytes ?? 65536,
          },
          { signal: scope.signal },
        );
        await presented(
          scope,
          response.items.flatMap((item) => item.sourceRefs),
        );
        return result(response);
      },
    },
  ];
}
