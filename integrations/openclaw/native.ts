import { RefSchema } from "@nous-wave/contracts/common";
import { NativeModelBindingSchema } from "@nous-wave/contracts/models";
// Native facts contract adapted from the Hept OpenClaw bridge, AGPL-3.0-only.
// Actual metadata and call facts remain owned by OpenClaw's published catalog and SessionManager.
import { z } from "zod";
export const bindingRead = z.strictObject({
  runtimeId: z.string().min(1),
  nativeResourceId: z.string().min(1),
});
export const modelBinding = NativeModelBindingSchema.omit({ id: true });
const count = z.number().int().nonnegative().safe();
export const callFacts = z.strictObject({
  agentId: z.string().min(1),
  sessionId: z.string().min(1),
  currentSessionId: z.string().min(1),
  sessionKey: z.string().min(1),
  runId: z.string().min(1),
  callId: z.string().min(1),
  prepared: z.strictObject({
    entryId: z.string().min(1),
    provider: z.string().min(1),
    modelId: z.string().min(1),
    role: z.string().optional(),
    inputTokensEstimate: count,
    outputTokens: count.positive(),
    inputRefs: z.array(RefSchema),
  }),
  active: z.boolean(),
  dispatched: z.boolean(),
  notDispatched: z.boolean(),
  terminal: z
    .strictObject({
      entryId: z.string().min(1),
      providerRefused: z.boolean(),
      outputRef: RefSchema.optional(),
      selectedOutputRefs: z.array(RefSchema).optional(),
      stopReason: z.string().optional(),
      providerRequestId: z.string().optional(),
      usage: z
        .strictObject({ input: count, output: count, cacheRead: count, cacheWrite: count })
        .optional(),
    })
    .optional(),
  settlement: z
    .strictObject({
      entryId: z.string().min(1),
      outcome: z.enum(["completed", "not_sent", "unknown"]),
    })
    .optional(),
});
export function resourceIdentity(input: { agentId: string; provider: string; modelId: string }) {
  return `agent:${encodeURIComponent(input.agentId)}:model:${encodeURIComponent(input.provider)}:${encodeURIComponent(input.modelId)}`;
}
export function parseResourceIdentity(value: string) {
  const match = /^agent:([^:]+):model:([^:]+):([^:]+)$/.exec(value);
  if (!match) throw new Error("Native resource identity must be an agent/provider/model tuple");
  const parsed = {
    agentId: decodeURIComponent(match[1]!),
    provider: decodeURIComponent(match[2]!),
    modelId: decodeURIComponent(match[3]!),
  };
  if (resourceIdentity(parsed) !== value)
    throw new Error("Native resource identity is not canonical");
  return parsed;
}
export function invocationIdentity(input: { runId: string; callId: string }) {
  const prefix = `${input.runId}:`;
  if (!input.callId.startsWith(prefix)) throw new Error("Native call does not belong to this run");
  return z.uuidv7().parse(input.callId.slice(prefix.length));
}
