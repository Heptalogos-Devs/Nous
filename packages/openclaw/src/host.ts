// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { open } from "node:fs/promises";
import { constants } from "node:fs";
import { isAbsolute } from "node:path";
import {
  HOST_BRIDGE_PATHS,
  HOST_ACTIVITY_CONTEXT_KEY,
  HOST_COGNITION_USAGE_CONTEXT_KEY,
  HOST_CONTACT_CONTEXT_KEY,
  decodeCognitionUsage,
  decodeObservationInput,
  encodeObservationInput,
  encodeAcceptedObservation,
  hostPrepareRequestSchema,
  hostPrepareResponseSchema,
  hostFeedbackResponseSchema,
  hostAcknowledgementSchema,
  hostFeedbackRequestSchema,
  hostHandoffRequestSchema,
  hostCompletionRequestSchema,
  hostCompletionResponseSchema,
  hostAdmittedRequestSchema,
  hostToolExecutionRequestSchema,
  hostToolExecutionResponseSchema,
  type HostPrepareRequest,
} from "@nous-wave/client/host-bridge";
import { z } from "zod";

export const hostBridgeConfiguration = z
  .object({ origin: z.string().url(), credentialFile: z.string().min(1) })
  .strict();
export class HostBridge {
  private readonly origin: string;
  constructor(
    private readonly config: z.infer<typeof hostBridgeConfiguration>,
  ) {
    const url = new URL(config.origin);
    if (
      url.protocol !== "http:" ||
      !["127.0.0.1", "[::1]"].includes(url.hostname) ||
      url.username ||
      url.password ||
      url.pathname !== "/" ||
      url.search ||
      url.hash ||
      !isAbsolute(config.credentialFile)
    )
      throw new Error(
        "Host bridge requires a configured loopback HTTP origin and absolute private credentialFile",
      );
    this.origin = url.origin;
  }
  private async call<S extends z.ZodType>(
    path: keyof typeof HOST_BRIDGE_PATHS,
    body: unknown,
    schema: S,
    signal?: AbortSignal,
  ): Promise<z.output<S>> {
    signal?.throwIfAborted();
    const file = await open(
      this.config.credentialFile,
      constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0),
    );
    let token: string;
    try {
      const stat = await file.stat();
      if (
        !stat.isFile() ||
        (stat.mode & 0o077) !== 0 ||
        (process.getuid && stat.uid !== process.getuid())
      )
        throw new Error(
          "Host bridge credential must be a private owner-only file",
        );
      token = (await file.readFile("utf8")).trim();
      if (!token || /[\r\n]/.test(token))
        throw new Error("Invalid Host bridge credential");
    } finally {
      await file.close();
    }
    const response = await fetch(this.origin + HOST_BRIDGE_PATHS[path], {
      method: "POST",
      redirect: "error",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
      signal: AbortSignal.any([
        ...(signal ? [signal] : []),
        AbortSignal.timeout(30000),
      ]),
    });
    if (!response.ok)
      throw new Error(`Host ${path} rejected (${response.status})`);
    return schema.parse(await response.json());
  }
  async prepare(request: HostPrepareRequest, signal: AbortSignal) {
    const response = await this.call(
      "prepare",
      hostPrepareRequestSchema.parse(request),
      hostPrepareResponseSchema,
      signal,
    );
    if (response.contactRequestId !== request.contactRequestId)
      throw new Error("Host contact identity mismatch");
    const usage = decodeCognitionUsage(response.usage);
    if (
      usage.agentInstanceId !==
        `${request.native.sessionId}:${request.native.runId}` ||
      usage.role !== response.roleScope.role ||
      usage.purpose !== response.roleScope.purpose ||
      usage.view !== response.roleScope.view ||
      usage.contactNature !== response.roleScope.contactNature ||
      !usage.audience ||
      !usage.intent.trim()
    )
      throw new Error("Host usage has missing or foreign native scope");
    const contacts = response.contacts.map((contact) => ({
      ...contact,
      observation: decodeHostObservation(contact.observation, request),
    }));
    return { response, usage, contacts };
  }
  async toolExecution(
    request: Parameters<typeof hostToolExecutionRequestSchema.parse>[0],
    signal: AbortSignal,
  ) {
    const input = hostToolExecutionRequestSchema.parse(request);
    const response = await this.call(
      "toolExecution",
      input,
      hostToolExecutionResponseSchema,
      signal,
    );
    if (
      response.contactRequestId !== input.contactRequestId ||
      response.toolName !== input.toolName ||
      response.toolCallId !== input.toolCallId
    )
      throw new Error("Host tool execution identity mismatch");
    return response;
  }
  async feedback(
    request: Parameters<typeof hostFeedbackRequestSchema.parse>[0],
    signal?: AbortSignal,
  ) {
    return this.call(
      "feedback",
      hostFeedbackRequestSchema.parse(request),
      hostFeedbackResponseSchema,
      signal,
    );
  }
  async admitted(
    request: Parameters<typeof hostAdmittedRequestSchema.parse>[0],
    signal: AbortSignal,
  ) {
    return this.call(
      "admitted",
      hostAdmittedRequestSchema.parse(request),
      hostAcknowledgementSchema,
      signal,
    );
  }
  async completion(
    request: Parameters<typeof hostCompletionRequestSchema.parse>[0],
  ) {
    return this.call(
      "completion",
      hostCompletionRequestSchema.parse(request),
      hostCompletionResponseSchema,
    );
  }
  async handoff(request: Parameters<typeof hostHandoffRequestSchema.parse>[0]) {
    return this.call(
      "handoff",
      hostHandoffRequestSchema.parse(request),
      hostAcknowledgementSchema,
    );
  }
}
export function decodeHostObservation(
  raw: Parameters<typeof decodeObservationInput>[0],
  identity: Pick<HostPrepareRequest, "subjectId" | "nousSessionId">,
) {
  const observation = decodeObservationInput(raw);
  if (
    observation.subjectId !== identity.subjectId ||
    observation.sessionId !== identity.nousSessionId ||
    !z.uuid().safeParse(observation.requestId).success ||
    !observation.observedAt ||
    observation.material.case === undefined
  )
    throw new Error(
      "Host observation has missing or foreign Subject/Session/contact facts",
    );
  if (
    typeof observation.context?.[HOST_ACTIVITY_CONTEXT_KEY] !== "string" ||
    !observation.context[HOST_ACTIVITY_CONTEXT_KEY] ||
    !z.uuid().safeParse(observation.context?.[HOST_CONTACT_CONTEXT_KEY])
      .success ||
    !observation.context?.[HOST_COGNITION_USAGE_CONTEXT_KEY]
  )
    throw new Error(
      "Host observation has no durable cognition/contact/Activity scope",
    );
  decodeCognitionUsage(observation.context[HOST_COGNITION_USAGE_CONTEXT_KEY]);
  return observation;
}
export const observationJson = encodeObservationInput;
export const occurrenceJson = encodeAcceptedObservation;
export type HostedPermission = Awaited<ReturnType<HostBridge["prepare"]>>;
