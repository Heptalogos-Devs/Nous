// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { Context, Model } from "@openclaw/ai";
import type { AppliedCognition } from "@nous-wave/client/managed";
import type { OpenClawPluginApi } from "openclaw/plugin-sdk/core";

type ContextEngine = Awaited<
  ReturnType<Parameters<OpenClawPluginApi["registerContextEngine"]>[1]>
>;
/** The required model-boundary extension carried by the controlled OpenClaw distribution. */
export interface ManagedModelInput {
  sessionId: string;
  sessionKey?: string;
  agentId: string;
  runId: string;
  prompt: string;
  model: Model;
  context: Context;
  signal: AbortSignal;
  contextTokenBudget: number;
  reservedOutputTokens: number;
  measureContext(context: Context): number;
  audience: {
    visibility: "direct" | "group" | "public" | "internal";
    channel?: string;
    senderId?: string;
    destination?: string;
  };
}
export interface ManagedModelApplication {
  context: Context;
  invocationId: string;
  applied: AppliedCognition;
}
export interface ManagedContextEngine extends ContextEngine {
  readonly info: ContextEngine["info"] & {
    managedModelInputRequired: true;
    transcriptPersistence: "host";
  };
  prepareModelInput(input: ManagedModelInput): Promise<ManagedModelApplication>;
  recordNativeEvent(
    input: ManagedModelInput,
    message: Context["messages"][number],
  ): Promise<void>;
  reportModelOutcome(
    input: ManagedModelInput,
    application: ManagedModelApplication,
    outcome: "completed" | "failed" | "cancelled",
    message?: Context["messages"][number],
  ): Promise<void>;
}
