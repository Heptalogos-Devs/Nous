import { ConsumerSchema } from "@nous-wave/contracts/common";
import type { NativeModelBinding, NativeInvocationPrepare } from "@nous-wave/contracts/models";
// Copyright 2026 Arsvine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { z } from "zod";
import { bindingRead, modelBinding, callFacts, resourceIdentity } from "./native.js";
export { resourceIdentity, invocationIdentity } from "./native.js";
export type NativeOrigin = NativeInvocationPrepare["native"];
export const scopedCallFacts = callFacts.extend({
  scope: z.strictObject({
    subjectId: z.string().min(1),
    consumer: ConsumerSchema,
    authorityRef: z.string().min(1),
    purpose: z.string().min(1),
    audience: z.array(z.string()),
    billing: z.literal("nous"),
    ancestors: z
      .array(z.strictObject({ sessionKey: z.string().min(1), sessionId: z.string().min(1) }))
      .min(1)
      .max(32),
  }),
});
export type NativeCallFacts = z.infer<typeof scopedCallFacts>;
export function createOpenClawModelClient(options: {
  endpoint: string;
  token: string;
  fetch?: typeof globalThis.fetch;
}) {
  async function request(operation: string, body: unknown, traceparent?: string) {
    const response = await (options.fetch ?? globalThis.fetch)(
      `${options.endpoint.replace(/\/$/, "")}/plugins/nous/models/${operation}`,
      {
        method: "POST",
        redirect: "error",
        signal: AbortSignal.timeout(120000),
        headers: {
          authorization: `Bearer ${options.token}`,
          "content-type": "application/json",
          ...(traceparent ? { traceparent } : {}),
        },
        body: JSON.stringify(body),
      },
    );
    if (!response.ok) throw new Error(`OpenClaw ${operation} failed (${response.status})`);
    return response.json();
  }
  return {
    readBinding: async (
      input: z.infer<typeof bindingRead>,
      traceparent?: string,
    ): Promise<Omit<NativeModelBinding, "id">> =>
      modelBinding.parse(await request("readBinding", bindingRead.parse({runtimeId:input.runtimeId,nativeResourceId:input.nativeResourceId}), traceparent)),
    readCall: async (input: NativeOrigin, traceparent?: string): Promise<NativeCallFacts | null> =>
      scopedCallFacts.nullable().parse(await request("readCall", input, traceparent)),
  };
}
