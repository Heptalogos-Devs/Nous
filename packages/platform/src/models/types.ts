// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only

import type { ProviderV4 } from '@ai-sdk/provider';
import type {
  CallContext,
  Consumer,
  Money,
  Problem,
  Ref,
} from '@nous-wave/contracts/common';
import type { SecretUse } from '@nous-wave/contracts/credentials';
import type {
  BudgetAllocationSnapshot,
  BudgetAllocationUse,
  InvocationSnapshot,
  ModelBudget,
  ModelCallView,
  ModelInvocationIdentity,
  ModelKind,
  ModelPreparation,
  NativeInvocationPrepare,
  NativeModelBinding,
  NormalizedUsage,
  PriceBook,
  ProviderDefinition,
  Reasoning,
  ResourceRef,
  RolePolicy,
  TokenEstimate,
} from '@nous-wave/contracts/models';
import type {
  ConfigScope,
  ConfigSnapshot,
  JsonObject,
} from '@nous-wave/contracts/settings';
import type {
  CapturedTime,
  TimeCaptureReceipt,
  TimeSelection,
} from '@nous-wave/contracts/time';
import type {
  Instructions,
  LanguageModel,
  ModelMessage,
  PrepareStepFunction,
  ToolSet,
} from 'ai';
import type { Kysely } from 'kysely';
import type { z } from 'zod';
import type { SecretUseMetadata } from '../credentials/secrets.js';
import type { Network, ResourceNetworkScope } from '../network/index.js';
import type { Prompts } from '../prompts/index.js';
import type { Settings, SettingsModule } from '../settings/index.js';
import type { ModelsDatabase } from './migration.js';

export * from '@nous-wave/contracts/models';

export interface RoleDescriptor {
  id: string;
  label: string;
  description: string;
  kind: ModelKind;
  requiredCapabilities: readonly string[];
  defaultPolicy: RolePolicy;
  promptId?: string;
  outputSchemaId?: string;
}
export interface ProviderAdapter {
  id: string;
  /** Complete nonsecret connection options. Never accepts credential headers. */
  optionsSchema: z.ZodType<JsonObject>;
  defaultBaseURL?: string;
  create(input: {
    provider: ProviderDefinition;
    apiKey?: string;
    fetch: typeof fetch;
  }): ProviderV4;
  networkScope(input: {
    provider: ProviderDefinition;
    resourceRef: Ref;
  }): ResourceNetworkScope;
  usage?(
    kind: ModelKind,
    value: unknown,
  ): Pick<TerminalMetadata, 'usage' | 'rawUsage'>;
  reasoning?(
    provider: ProviderDefinition,
    value: Exclude<Reasoning, 'provider-default'>,
  ): Record<string, JsonObject>;
}
export interface ModelCredentials {
  withSecret<T>(
    ctx: CallContext,
    input: SecretUse,
    use: (bytes: Uint8Array, metadata: SecretUseMetadata) => Promise<T>,
  ): Promise<T>;
}
export interface ModelClock {
  read(ctx: CallContext, selection?: TimeSelection): Promise<CapturedTime>;
  capture(
    ctx: CallContext,
    input: { operationId: string; selection?: TimeSelection; realAt?: string },
  ): Promise<TimeCaptureReceipt>;
  captureTerminal(
    ctx: CallContext,
    input: {
      operationId: string;
      originRef: Ref;
      consumer: Consumer;
      observed: TimeCaptureReceipt;
      realAt?: string;
    },
  ): Promise<TimeCaptureReceipt>;
  validateCapture(
    ctx: CallContext,
    receipt: TimeCaptureReceipt,
  ): Promise<TimeCaptureReceipt>;
}
export interface BudgetConstraint {
  key: string;
  budget: ModelBudget;
}
export interface BudgetAdmission {
  identity: ModelInvocationIdentity;
  snapshot: InvocationSnapshot;
  reservedCost: Money | null;
  estimate: TokenEstimate;
  maxOutputTokens: number;
}
export interface ModelBudgetScopes {
  /** Resolve current domain limits/allocation outside the SQL transaction. */
  resolve(
    ctx: CallContext,
    input: { roleId: string; allocation?: BudgetAllocationUse },
  ): Promise<{
    constraints: BudgetConstraint[];
    allocation?: BudgetAllocationSnapshot;
  }>;
  /** Same-product Task owner can reserve with this exact short transaction. No network waits. */
  admit?(
    ctx: CallContext,
    input: BudgetAdmission,
    db: Kysely<ModelsDatabase>,
  ): Promise<void>;
  settle?(ctx: CallContext, call: ModelCallView): Promise<void>;
}
export interface ModelData {
  validateSources(ctx: CallContext, refs: readonly Ref[]): Promise<void>;
  saveInput(
    ctx: CallContext,
    input: {
      identity: ModelInvocationIdentity;
      snapshot: InvocationSnapshot;
      value: unknown;
      inputRefs: readonly Ref[];
      observed: TimeCaptureReceipt;
      preparation?: ModelPreparation;
    },
  ): Promise<Ref[]>;
  /** Save actual raw Provider output before forming/adopting its semantic result. Handles Blob bytes. */
  saveOutput(
    ctx: CallContext,
    input: {
      identity: ModelInvocationIdentity;
      snapshot: InvocationSnapshot;
      value: unknown;
      inputRefs: readonly Ref[];
      realAt: string;
    },
  ): Promise<Ref[]>;
  loadOutput(
    ctx: CallContext,
    input: { identity: ModelInvocationIdentity; outputRefs: readonly Ref[] },
  ): Promise<unknown>;
  adopt(
    ctx: CallContext,
    input: {
      call: ModelCallView;
      inputRefs: readonly Ref[];
      outputRefs: readonly Ref[];
    },
  ): Promise<void>;
}
export interface ModelContextHooks {
  prepare(
    ctx: CallContext,
    input: {
      snapshot: InvocationSnapshot;
      stepNumber: number;
      messages: ModelMessage[];
      inputRefs: readonly Ref[];
      tools: ToolSet;
      model: LanguageModel;
      previousPreparation?: import('@nous-wave/contracts/models').ModelPreparation;
      baseInstructions: Instructions;
      referenceTime: CapturedTime;
    },
  ): Promise<{ instructions?: string; inputRefs: Ref[]; contextRef?: Ref }>;
}
/** Trusted local caller for native transport; never a wire field or persisted configuration. */
export interface NativeHostContext {
  hostContext: CallContext;
}
export interface ModelsOptions {
  product: 'nous' | 'hept';
  installationId: string;
  processBootId: string;
  db: Kysely<ModelsDatabase>;
  settings: Settings;
  credentials: ModelCredentials;
  network: Network;
  prompts: Pick<Prompts, 'snapshot' | 'render'>;
  roleCatalog: readonly RoleDescriptor[];
  providerAdapters: readonly ProviderAdapter[];
  authorize(
    ctx: CallContext,
    operation: string,
    target?: Ref,
    client?: Kysely<any>,
  ): Promise<void>;
  clock: ModelClock;
  data: ModelData;
  budgetScopes: ModelBudgetScopes;
  beforeDispatch?(): void | Promise<void>;
  nativeBindings?: {
    read(
      ctx: CallContext,
      input: { runtimeId: string; nativeResourceId: string },
      local?: NativeHostContext,
    ): Promise<Omit<NativeModelBinding, 'id'>>;
    authorizeCall(
      ctx: CallContext,
      input: {
        invocationId: string;
        resource: ResourceRef;
        native: NativeInvocationPrepare['native'];
        phase: 'prepare' | 'dispatch' | 'complete';
      },
      local?: NativeHostContext,
    ): Promise<void>;
  };
  contextHooks?: ModelContextHooks;
  diagnostic?(ctx: CallContext, problem: Problem): void;
}
export interface PrepareModelOptions {
  nativeHost?: NativeHostContext;
  resource?: ResourceRef;
  budgetAllocation?: BudgetAllocationUse;
  promptParams?: unknown;
}
export interface PreparedModel {
  snapshot: InvocationSnapshot;
  instructions?: string;
  constraints: BudgetConstraint[];
}
export interface ModelCallOptions {
  roleId: string;
  inputRefs: Ref[];
  prepared?: InvocationSnapshot;
  resource?: ResourceRef;
  budgetAllocation?: BudgetAllocationUse;
  promptParams?: unknown;
  estimate?: TokenEstimate;
  selection?: TimeSelection;
  preparation?: ModelPreparation;
  /** Trusted host closure; never a wire/model input or a persisted setting. */
  contextHooks?: ModelContextHooks;
}
export type HostPrepareStep = PrepareStepFunction<ToolSet>;
export interface ModelSettingsDefinitions {
  modules: SettingsModule[];
  providers: string;
  resources: string;
  roles: string;
  prices: string;
  budgets: string;
  limits: string;
}
export interface ConfigurationRead {
  providers: ConfigSnapshot;
  resources: ConfigSnapshot;
  roles: ConfigSnapshot;
  prices: ConfigSnapshot;
  budgets: ConfigSnapshot;
  limits: { scope: ConfigScope; snapshot: ConfigSnapshot }[];
  scope: ConfigScope;
}
export interface TerminalMetadata {
  usage?: NormalizedUsage;
  rawUsage?: unknown;
  providerRequestId?: string;
  problem?: Problem;
}
