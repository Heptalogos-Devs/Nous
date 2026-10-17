// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { Context, Model } from "@openclaw/ai";
import type {
  HostContactIdentity,
  HostRoleScope,
  HostResponseFormat,
  HostRunTerminalRequest,
  HostRunTerminalResponse,
} from "@nous-wave/client/host-bridge";
import type { AppliedCognition } from "@nous-wave/client/managed";
import type { OpenClawPluginApi } from "openclaw/plugin-sdk/core";

type ContextEngine = Awaited<
  ReturnType<Parameters<OpenClawPluginApi["registerContextEngine"]>[1]>
>;
/** The required model-boundary extension carried by the controlled OpenClaw distribution. */
export interface ManagedModelInput {
  boundaryVersion: 5;
  historyIsFresh: boolean;
  historyGeneration: string;
  persistSessionBinding(value: Record<string, string>): Promise<void>;
  persistAdmittedContact?(
    identity: HostContactIdentity,
    roleScope: HostRoleScope,
  ): Promise<void>;
  sessionId: string;
  sessionKey?: string;
  parentSessionKey?: string;
  agentId: string;
  runId: string;
  prompt: string;
  /** Exact authored per-run system addition, before native framework hints; excluded from transcript history. */
  extraSystemPrompt?: string;
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
  /** Applied by the controlled native stream owner after all generic options. */
  providerControls: {
    maxOutputTokens: number;
    responseFormat?: HostResponseFormat;
  };
}
export interface ManagedToolExecution {
  readonly toolName: string;
  readonly toolCallId: string;
  readonly phase: "prepare" | "execute";
}
export interface ManagedToolExecutionGuard {
  run<T>(implementation: () => T | Promise<T>): Promise<T>;
}
/** Carried only by the native durable child terminal/announcement owner. */
export interface NativeSubagentCompletion {
  parentRunId?: string;
  parentSessionKey: string;
  childRunId: string;
  childSessionKey: string;
  outcome: "completed" | "failed" | "cancelled";
  endedAt: string;
  recover?: boolean;
}
export interface ManagedContextEngine extends ContextEngine {
  runTerminal(
    request: HostRunTerminalRequest,
  ): Promise<HostRunTerminalResponse | undefined>;
  prepareSubagentCompletion(input: NativeSubagentCompletion): Promise<
    | {
        parentRunId: string;
        resultRetained: true;
      }
    | undefined
  >;
  authorizeToolExecution(
    input: ManagedModelInput,
    application: ManagedModelApplication,
    execution: ManagedToolExecution,
  ): Promise<ManagedToolExecutionGuard>;
  readonly info: ContextEngine["info"] & {
    managedModelInputRequired: true;
    transcriptPersistence: "host";
  };
  prepareSubagentSpawn(
    params: Parameters<
      NonNullable<ContextEngine["prepareSubagentSpawn"]>
    >[0] & {
      /** Exact native run that requested this child, carried by the native tool context. */
      parentRunId: string;
      childExtraSystemPrompt?: string;
      persistSessionBinding(value: Record<string, string>): Promise<void>;
    },
  ): Promise<{ rollback(): Promise<void> }>;
  captureNativeEvent(
    input: ManagedModelInput,
    message: Context["messages"][number],
    application?: ManagedModelApplication,
    outcome?: "completed" | "failed" | "cancelled",
  ): void;
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

/** The pinned runtime patch supplies this export; source checks also run before
 * preparation, against the pristine upstream package declarations. */
declare module "openclaw/plugin-sdk/core" {
  function retryNousSubagentCompletion(params: {
    childRunId: string;
    childSessionKey: string;
    expectedEndedAt: number;
  }): Promise<{ accepted: true; childRunId: string }>;
}
