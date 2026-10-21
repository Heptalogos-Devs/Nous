// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only

import { DBOS } from '@dbos-inc/dbos-sdk';
import {
  embed,
  embedMany,
  generateSpeech,
  generateText,
  type LanguageModel,
  type ModelMessage,
  type OutputInterface,
  type PrepareStepFunction,
  rerank,
  streamText,
  type ToolSet,
  transcribe,
} from 'ai';
import { createModelDataLifecycle } from './data.js';

export { type ModelDataBundle, ModelDataBundleSchema } from './data.js';

import deepmerge from '@fastify/deepmerge';
import {
  type CallContext,
  canonicalRefKey,
  type Ref,
} from '@nous-wave/contracts/common';
import {
  InvocationSnapshotSchema,
  ModelPreparationSchema,
  NativeInvocationCompletionSchema,
  NativeInvocationPrepareSchema,
  NativeOriginSchema,
  type ResourceRef,
} from '@nous-wave/contracts/models';
import { problem } from '../errors.js';
import { digest, immutable, parse } from '../settings/catalog.js';
import { createModelBoundary } from './boundary.js';
import {
  createModelConfiguration,
  modelNamespaces,
  modelSettingsModules,
} from './configuration.js';
import { providerError, providerProblem } from './errors.js';
import { createInvocationOwner } from './invocations.js';
import type {
  ModelCallOptions,
  ModelsOptions,
  NativeHostContext,
  PreparedModel,
  PrepareModelOptions,
} from './types.js';

export { officialProviderAdapters } from './adapters.js';
export { type ModelsDatabase, modelMigration } from './migration.js';
export * from './types.js';
export { languageUsage, modelCost, nativeLanguageUsage } from './usage.js';
export { modelNamespaces, modelSettingsModules };

type Without<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
type SdkTextOptions<
  TOOLS extends ToolSet,
  OUTPUT extends OutputInterface,
> = Without<
  Parameters<typeof generateText<TOOLS, Record<string, unknown>, OUTPUT>>[0],
  'model' | 'maxRetries'
>;
type SdkStreamOptions<
  TOOLS extends ToolSet,
  OUTPUT extends OutputInterface,
> = Without<
  Parameters<typeof streamText<TOOLS, Record<string, unknown>, OUTPUT>>[0],
  'model' | 'maxRetries'
>;
export type TextRequest<
  TOOLS extends ToolSet = ToolSet,
  OUTPUT extends OutputInterface = OutputInterface,
> = ModelCallOptions & { options: SdkTextOptions<TOOLS, OUTPUT> };
export type TextStreamRequest<
  TOOLS extends ToolSet = ToolSet,
  OUTPUT extends OutputInterface = OutputInterface,
> = ModelCallOptions & { options: SdkStreamOptions<TOOLS, OUTPUT> };
type SdkEmbedOptions = Omit<
  Parameters<typeof embed>[0],
  'model' | 'maxRetries'
>;
type SdkEmbedManyOptions = Omit<
  Parameters<typeof embedMany>[0],
  'model' | 'maxRetries'
>;
type SdkRerankOptions = Omit<
  Parameters<typeof rerank>[0],
  'model' | 'maxRetries'
>;
type SdkTranscribeOptions = Omit<
  Parameters<typeof transcribe>[0],
  'model' | 'maxRetries'
>;
type SdkSpeechOptions = Omit<
  Parameters<typeof generateSpeech>[0],
  'model' | 'maxRetries'
>;

export function createModels(options: ModelsOptions) {
  const configuration = createModelConfiguration(options),
    invocations = createInvocationOwner(options),
    boundary = createModelBoundary(options, configuration, invocations);
  let closed = false;
  const active = () => {
    if (closed) throw problem('models.closed', 'unavailable', '模型库已关闭');
  };
  const bindings = new WeakMap<
    object,
    {
      prepared: PreparedModel;
      request: ModelCallOptions;
      baseInputRefs: Ref[];
      stepNo: number;
    }
  >();
  const merge = deepmerge({
    mergeArray:
      ({ clone }) =>
      (_previous, next) =>
        clone(next),
  });
  async function authorizeNative(
    ctx: CallContext,
    row: Awaited<ReturnType<typeof invocations.fact>>,
    phase: 'prepare' | 'dispatch' | 'complete',
    local?: NativeHostContext,
  ) {
    if (!options.nativeBindings?.authorizeCall)
      throw problem(
        'models.native_owner_unavailable',
        'unavailable',
        '实际原生调用授权owner未配置',
      );
    if (row.subjectId !== (ctx.subjectId ?? null))
      throw problem(
        'models.native_scope',
        'forbidden',
        '原生调用不属于当前主体范围',
      );
    const native = parse(NativeOriginSchema, row.nativeIdentity);
    await options.nativeBindings.authorizeCall(
      ctx,
      {
        invocationId: row.id,
        resource: row.configSnapshot.actualResource,
        native,
        phase,
      },
      local,
    );
  }
  async function prepared(
    ctx: CallContext,
    request: ModelCallOptions,
    kind: string,
    ordinal?: number,
  ) {
    active();
    let selected: PreparedModel;
    let query = options.db
      .selectFrom('modelInvocations')
      .selectAll()
      .where('logicalOperationId', '=', ctx.trace.operationId)
      .where('roleId', '=', request.roleId);
    if (ordinal !== undefined)
      query = query.where('physicalOrdinal', '=', ordinal);
    if (request.resource)
      query = query.where('resourceId', '=', request.resource.id);
    const prior = await query
      .orderBy('physicalOrdinal')
      .orderBy('attemptNo')
      .executeTakeFirst();
    if (request.prepared || prior) {
      const snapshot = parse(
        InvocationSnapshotSchema,
        request.prepared ?? prior!.configSnapshot,
      );
      if (snapshot.roleId !== request.roleId || snapshot.policy.kind !== kind)
        throw problem(
          'models.snapshot_mismatch',
          'conflict',
          '冻结调用快照与本次职责不一致',
        );
      if (prior) {
        if (
          prior.subjectId !== (ctx.subjectId ?? null) ||
          digest(prior.consumer) !== digest(ctx.consumer)
        )
          throw problem(
            'models.invocation_scope',
            'forbidden',
            '原调用的主体或消费者不匹配',
          );
        await options.authorize(ctx, 'models.invocations.read', {
          product: options.product,
          installationId: options.installationId,
          ...(ctx.subjectId ? { subjectId: ctx.subjectId } : {}),
          kind: 'ModelInvocation',
          id: prior.id,
        });
        await options.data.validateSources(ctx, prior.inputSources);
      }
      if (!prior || prior.status === 'prepared')
        await configuration.assertCurrent(ctx, snapshot);
      let instructions: string | undefined;
      if (snapshot.prompt)
        instructions = (
          await options.prompts.render(ctx, {
            snapshot: snapshot.prompt,
            params: request.promptParams ?? {},
          })
        ).text;
      selected = {
        snapshot,
        instructions,
        constraints:
          prior?.budgetSnapshot.constraints ??
          (
            await options.budgetScopes.resolve(ctx, {
              roleId: request.roleId,
              allocation: request.budgetAllocation,
            })
          ).constraints,
      };
    } else selected = await configuration.prepare(ctx, request.roleId, request);
    if (selected.snapshot.policy.kind !== kind)
      throw problem(
        'models.kind_mismatch',
        'validation',
        '模型职责不支持本次操作',
      );
    if (selected.snapshot.definition.managedBy !== 'sdk')
      throw problem(
        'models.native_execution_required',
        'validation',
        '原生资源必须通过实际原生调用入口执行',
      );
    return prior ? selected : boundary.freeze(ctx, selected);
  }
  async function bind(
    ctx: CallContext,
    input: ModelCallOptions,
    ordinal?: number,
  ) {
    const request = { ...input, inputRefs: [...input.inputRefs] },
      selected = await prepared(ctx, request, 'language', ordinal),
      entry = {
        prepared: selected,
        request,
        baseInputRefs: [...input.inputRefs],
        stepNo: ordinal ?? 0,
      };
    const model = await boundary.language(
      ctx,
      selected,
      request,
      () => entry.stepNo,
    );
    bindings.set(model, entry);
    return { model, snapshot: selected.snapshot, prepared: selected, entry };
  }
  function instructions(...values: unknown[]) {
    const result: { role: 'system'; content: string }[] = [];
    for (const value of values) {
      if (typeof value === 'string' && value)
        result.push({ role: 'system', content: value });
      else if (Array.isArray(value)) result.push(...value);
      else if (value && typeof value === 'object')
        result.push(value as { role: 'system'; content: string });
    }
    return result;
  }
  function languageOptions(
    selected: PreparedModel,
    sdk: Record<string, unknown>,
    host: Record<string, unknown> = {},
  ) {
    const policy = selected.snapshot.policy;
    if (policy.kind !== 'language')
      throw problem('models.kind_mismatch', 'validation', '职责不是语言模型');
    const resource =
      selected.snapshot.definition.managedBy === 'sdk'
        ? selected.snapshot.definition.definition
        : undefined;
    const level = policy.inference.reasoning,
      adapter = options.providerAdapters.find(
        (adapter) => adapter.id === selected.snapshot.provider?.adapter,
      );
    if (level && level !== 'provider-default' && !adapter?.reasoning)
      throw problem(
        'models.reasoning_unavailable',
        'unavailable',
        '当前Provider适配器没有该推理设置映射，请使用其正式ProviderOptions',
      );
    const reasoning =
      level && level !== 'provider-default'
        ? adapter!.reasoning!(selected.snapshot.provider!, level)
        : {};
    return {
      maxOutputTokens: (host.maxOutputTokens ??
        sdk.maxOutputTokens ??
        policy.inference.maxOutputTokens) as number,
      temperature: (host.temperature ??
        sdk.temperature ??
        policy.inference.temperature) as number | undefined,
      providerOptions: merge(
        merge(
          merge(
            merge(resource?.providerOptions ?? {}, reasoning),
            policy.inference.providerOptions,
          ),
          sdk.providerOptions ?? {},
        ),
        host.providerOptions ?? {},
      ) as import('@ai-sdk/provider').SharedV4ProviderOptions,
    };
  }
  async function textOptions<
    TOOLS extends ToolSet,
    OUTPUT extends OutputInterface,
  >(
    ctx: CallContext,
    request: TextRequest<TOOLS, OUTPUT> | TextStreamRequest<TOOLS, OUTPUT>,
  ) {
    const requestRefs = request.inputRefs,
      bound = await bind(ctx, request),
      sdk = request.options;
    const prepareStep: PrepareStepFunction<
      TOOLS,
      Record<string, unknown>
    > = async (step) => {
      const host = await sdk.prepareStep?.(step),
        requestedModel = host?.model ?? bound.model;
      if (
        typeof requestedModel !== 'object' ||
        requestedModel === null ||
        !bindings.has(requestedModel)
      )
        throw problem(
          'models.unregistered_step_model',
          'forbidden',
          '本步模型必须由正式模型库绑定',
        );
      const original = bindings.get(requestedModel)!;
      const previous = await options.db
        .selectFrom('modelInvocations')
        .selectAll()
        .where('logicalOperationId', '=', ctx.trace.operationId)
        .where('physicalOrdinal', '=', step.stepNumber)
        .where('attemptNo', '=', 1)
        .executeTakeFirst();
      const current = await bind(
        ctx,
        {
          ...original.request,
          inputRefs: original.baseInputRefs,
          prepared: previous?.configSnapshot,
          resource: host?.model
            ? original.prepared.snapshot.requestedResource
            : original.request.resource,
          preparation: undefined,
        },
        step.stepNumber,
      );
      const entry = current.entry,
        selected = entry.prepared,
        model = current.model,
        messages = host?.messages ?? step.messages;
      const names = host?.activeTools ?? sdk.activeTools;
      const tools = (
        Array.isArray(names)
          ? Object.fromEntries(
              Object.entries(sdk.tools ?? {}).filter(([name]) =>
                names.includes(name as never),
              ),
            )
          : (sdk.tools ?? {})
      ) as ToolSet;
      const actual = languageOptions(selected, sdk, host ?? {});
      const contextSnapshot = parse(InvocationSnapshotSchema, {
        ...selected.snapshot,
        inference: {
          ...selected.snapshot.inference,
          ...Object.fromEntries(
            Object.entries(actual).filter(([, value]) => value !== undefined),
          ),
        },
      });
      const baseInstructions = immutable(
        structuredClone(
          instructions(
            selected.instructions,
            host?.instructions ??
              host?.system ??
              sdk.instructions ??
              sdk.system,
          ),
        ),
      );
      const previousPreparation = previous?.preparation
        ? immutable(parse(ModelPreparationSchema, previous.preparation))
        : undefined;
      const referenceTime =
        previousPreparation?.referenceTime ??
        immutable(await options.clock.read(ctx, request.selection));
      const hooks =
        request.contextHooks ??
        entry.request.contextHooks ??
        options.contextHooks;
      const contribution = hooks
        ? await hooks.prepare(ctx, {
            snapshot: contextSnapshot,
            stepNumber: step.stepNumber,
            messages: messages as ModelMessage[],
            inputRefs: [...entry.baseInputRefs, ...requestRefs],
            tools,
            model,
            baseInstructions,
            referenceTime,
            previousPreparation,
          })
        : undefined;
      entry.request.inputRefs = [
        ...new Map(
          [
            ...entry.baseInputRefs,
            ...requestRefs,
            ...(contribution?.inputRefs ?? []),
          ].map((ref) => [canonicalRefKey(ref), ref]),
        ).values(),
      ];
      entry.request.preparation = parse(ModelPreparationSchema, {
        referenceTime,
        ...(contribution?.contextRef
          ? { contextRef: contribution.contextRef }
          : {}),
      });
      return {
        ...actual,
        ...host,
        providerOptions: actual.providerOptions,
        model,
        messages,
        instructions: instructions(
          baseInstructions,
          contribution?.instructions,
        ),
      };
    };
    return {
      sdk: {
        ...languageOptions(bound.prepared, sdk),
        model: bound.model,
        prepareStep,
        maxRetries: 0,
        timeout: sdk.timeout ?? bound.snapshot.policy.timeoutMs,
      },
      snapshot: bound.snapshot,
    };
  }
  const api = {
    data: createModelDataLifecycle(options),
    providers: configuration.providers,
    resources: configuration.resources,
    credentialTarget: configuration.credentialTarget,
    networkSelection: configuration.networkSelection,
    status: configuration.status,
    describeRoles: configuration.describeRoles,
    async prepare(
      ctx: CallContext,
      roleId: string,
      input: PrepareModelOptions = {},
    ) {
      active();
      return (
        await boundary.freeze(
          ctx,
          await configuration.prepare(ctx, roleId, input),
        )
      ).snapshot;
    },
    text: {
      /** Metadata-only V4 binding for an SDK host's real prepareStep.model selection. */
      async bind(ctx: CallContext, request: ModelCallOptions) {
        return (await bind(ctx, request)).model;
      },
      async generate<TOOLS extends ToolSet, OUTPUT extends OutputInterface>(
        ctx: CallContext,
        request: TextRequest<TOOLS, OUTPUT>,
      ) {
        const selected = await textOptions(ctx, request);
        try {
          return await generateText<TOOLS, Record<string, unknown>, OUTPUT>({
            ...request.options,
            ...selected.sdk,
          } as Parameters<
            typeof generateText<TOOLS, Record<string, unknown>, OUTPUT>
          >[0]);
        } catch (error) {
          throw providerError(error);
        }
      },
      async stream<TOOLS extends ToolSet, OUTPUT extends OutputInterface>(
        ctx: CallContext,
        request: TextStreamRequest<TOOLS, OUTPUT>,
      ) {
        const selected = await textOptions(ctx, request);
        return streamText<TOOLS, Record<string, unknown>, OUTPUT>({
          ...request.options,
          ...selected.sdk,
          streamRetries: undefined,
          onError: async ({ error }: { error: unknown }) => {
            const safe = providerError(error);
            options.diagnostic?.(ctx, providerProblem(safe));
            await request.options.onError?.({ error: safe });
          },
        } as Parameters<
          typeof streamText<TOOLS, Record<string, unknown>, OUTPUT>
        >[0]);
      },
    },
    embeddings: {
      async embed(
        ctx: CallContext,
        request: ModelCallOptions & { options: SdkEmbedOptions },
      ) {
        const selected = await prepared(ctx, request, 'embedding'),
          model = await boundary.embedding(ctx, selected, request, [
            request.options.value,
          ]);
        try {
          return await embed({
            ...request.options,
            providerOptions: {
              ...selected.snapshot.policy.inference.providerOptions,
              ...request.options.providerOptions,
            },
            model,
            maxRetries: 0,
          });
        } catch (error) {
          throw providerError(error);
        }
      },
      async embedMany(
        ctx: CallContext,
        request: ModelCallOptions & { options: SdkEmbedManyOptions },
      ) {
        const selected = await prepared(ctx, request, 'embedding'),
          model = await boundary.embedding(
            ctx,
            selected,
            request,
            request.options.values,
          );
        const policy = selected.snapshot.policy;
        if (policy.kind !== 'embedding')
          throw problem(
            'models.kind_mismatch',
            'validation',
            '职责不是向量模型',
          );
        try {
          return await embedMany({
            ...request.options,
            providerOptions: {
              ...policy.inference.providerOptions,
              ...request.options.providerOptions,
            },
            maxParallelCalls:
              request.options.maxParallelCalls ??
              policy.inference.maxParallelCalls,
            model,
            maxRetries: 0,
          });
        } catch (error) {
          throw providerError(error);
        }
      },
    },
    reranking: {
      async rerank(
        ctx: CallContext,
        request: ModelCallOptions & { options: SdkRerankOptions },
      ) {
        const selected = await prepared(ctx, request, 'reranking'),
          family = boundary.family(ctx, selected, request, 'reranking'),
          metadata = await family.metadata();
        const model = {
          specificationVersion: 'v4' as const,
          ...metadata,
          doRerank: (
            params: Parameters<
              import('@ai-sdk/provider').RerankingModelV4['doRerank']
            >[0],
          ) =>
            family.invoke(
              params,
              (sdk) => sdk.doRerank(params),
              (raw) => ({ providerRequestId: raw.response?.id }),
            ),
        };
        const policy = selected.snapshot.policy;
        if (policy.kind !== 'reranking')
          throw problem(
            'models.kind_mismatch',
            'validation',
            '职责不是重排模型',
          );
        return rerank({
          ...request.options,
          topN: request.options.topN ?? policy.inference.topN,
          providerOptions: {
            ...policy.inference.providerOptions,
            ...request.options.providerOptions,
          },
          model,
          maxRetries: 0,
        });
      },
    },
    transcription: {
      async transcribe(
        ctx: CallContext,
        request: ModelCallOptions & { options: SdkTranscribeOptions },
      ) {
        const selected = await prepared(ctx, request, 'transcription'),
          family = boundary.family(ctx, selected, request, 'transcription'),
          metadata = await family.metadata();
        const model = {
          specificationVersion: 'v4' as const,
          ...metadata,
          doGenerate: (
            params: Parameters<
              import('@ai-sdk/provider').TranscriptionModelV4['doGenerate']
            >[0],
          ) =>
            family.invoke(
              params,
              (sdk) => sdk.doGenerate(params),
              (raw) => ({
                rawUsage: raw.usage,
                ...(raw.durationInSeconds === undefined
                  ? {}
                  : {
                      usage: {
                        units: [
                          {
                            unit: 'audio_seconds',
                            quantity: String(raw.durationInSeconds),
                            source: 'measured' as const,
                          },
                        ],
                      },
                    }),
              }),
            ),
        };
        return transcribe({
          ...request.options,
          providerOptions: {
            ...selected.snapshot.policy.inference.providerOptions,
            ...request.options.providerOptions,
          },
          model,
          maxRetries: 0,
        });
      },
    },
    speech: {
      async generate(
        ctx: CallContext,
        request: ModelCallOptions & { options: SdkSpeechOptions },
      ) {
        const selected = await prepared(ctx, request, 'speech'),
          family = boundary.family(ctx, selected, request, 'speech'),
          metadata = await family.metadata();
        const model = {
          specificationVersion: 'v4' as const,
          ...metadata,
          doGenerate: (
            params: Parameters<
              import('@ai-sdk/provider').SpeechModelV4['doGenerate']
            >[0],
          ) =>
            family.invoke(
              params,
              (sdk) => sdk.doGenerate(params),
              (raw) => ({
                rawUsage: raw.usage,
                usage: {
                  units: [
                    {
                      unit: 'text_characters',
                      quantity: String([...params.text].length),
                      source: 'measured' as const,
                    },
                  ],
                },
              }),
            ),
        };
        const policy = selected.snapshot.policy;
        if (policy.kind !== 'speech')
          throw problem(
            'models.kind_mismatch',
            'validation',
            '职责不是语音模型',
          );
        return generateSpeech({
          ...policy.inference,
          ...request.options,
          providerOptions: {
            ...policy.inference.providerOptions,
            ...request.options.providerOptions,
          },
          model,
          maxRetries: 0,
        });
      },
    },
    native: {
      async prepare(
        ctx: CallContext,
        input: unknown,
        local?: NativeHostContext,
      ) {
        active();
        await options.beforeDispatch?.();
        const request = parse(NativeInvocationPrepareSchema, input);
        if (!options.nativeBindings?.authorizeCall)
          throw problem(
            'models.native_owner_unavailable',
            'unavailable',
            '实际原生调用授权owner未配置',
          );
        const prior = await options.db
          .selectFrom('modelInvocations')
          .select('id')
          .where('id', '=', request.invocationId)
          .executeTakeFirst();
        if (prior) {
          const row = await invocations.own(ctx, prior.id);
          if (
            !row.nativeIdentity ||
            row.logicalOperationId !== ctx.trace.operationId ||
            row.roleId !== request.roleId ||
            row.resourceId !== request.modelResourceId ||
            row.actualProvider !== request.actualProvider ||
            row.actualModelId !== request.actualModelId ||
            digest(row.nativeIdentity) !== digest(request.native) ||
            digest(row.inputSources) !== digest(request.inputRefs) ||
            digest(row.tokenEstimate) !== digest(request.inputTokensEstimate) ||
            row.maxOutputTokens !== request.maxOutputTokens
          )
            throw problem(
              'models.submission_conflict',
              'conflict',
              '同一原生调用身份包含不同输入',
            );
          await options.nativeBindings.authorizeCall(
            ctx,
            {
              invocationId: row.id,
              resource: row.configSnapshot.actualResource,
              native: request.native,
              phase: 'prepare',
            },
            local,
          );
          return invocations.view(row);
        }
        const resource = await configuration.nativeResource(
          ctx,
          request.roleId,
          request.modelResourceId,
        );
        const selected = await configuration.prepare(ctx, request.roleId, {
          resource,
          budgetAllocation: request.budgetAllocation,
          nativeHost: local,
        });
        const snapshot = selected.snapshot;
        if (
          snapshot.definition.managedBy !== 'native' ||
          snapshot.actualResource.id !== request.modelResourceId ||
          snapshot.actualProvider !== request.actualProvider ||
          snapshot.actualModelId !== request.actualModelId ||
          snapshot.definition.binding.runtimeId !== request.native.runtimeId
        )
          throw problem(
            'models.native_identity_mismatch',
            'forbidden',
            '实际原生模型与已授权资源绑定不一致',
          );
        await options.nativeBindings.authorizeCall(
          ctx,
          {
            invocationId: request.invocationId,
            resource: snapshot.actualResource,
            native: request.native,
            phase: 'prepare',
          },
          local,
        );
        if (request.inputRefs.some((ref) => ref.kind === 'ContextView'))
          throw problem(
            'models.context_as_source',
            'validation',
            'ContextView必须保存为准备元数据',
          );
        await options.data.validateSources(ctx, request.inputRefs);
        return invocations.prepare(ctx, {
          identity: {
            invocationId: request.invocationId,
            logicalOperationId: ctx.trace.operationId,
            physicalOrdinal: 0,
            attemptNo: 1,
            ...(DBOS.isWithinWorkflow() ? { workflowId: DBOS.workflowID } : {}),
          },
          snapshot,
          constraints: selected.constraints,
          inputRefs: request.inputRefs,
          semanticInputRefs: request.inputRefs,
          estimate: request.inputTokensEstimate,
          maxOutputTokens: request.maxOutputTokens,
          native: request.native,
          preparation: request.preparation,
        });
      },
      async dispatch(
        ctx: CallContext,
        input: {
          invocationId: string;
          observed?: import('@nous-wave/contracts/time').TimeCaptureReceipt;
        },
        local?: NativeHostContext,
      ) {
        active();
        await options.beforeDispatch?.();
        const row = await invocations.own(ctx, input.invocationId);
        if (!row.nativeIdentity)
          throw problem(
            'models.native_call_required',
            'validation',
            '该调用由SDK拥有',
          );
        await authorizeNative(ctx, row, 'dispatch', local);
        await configuration.assertCurrent(ctx, row.configSnapshot, local);
        const allocation = row.configSnapshot.budgetAllocation;
        const fresh = await options.budgetScopes.resolve(ctx, {
          roleId: row.roleId,
          ...(allocation
            ? {
                allocation: {
                  allocationRef: allocation.allocationRef,
                  childOperationId: allocation.childOperationId,
                },
              }
            : {}),
        });
        if (
          row.configSnapshot.budgetAllocation &&
          digest(fresh.allocation) !==
            digest(row.configSnapshot.budgetAllocation)
        )
          throw problem(
            'models.allocation_changed',
            'forbidden',
            '根Task子额度当前状态已变化',
          );
        await invocations.recheck(ctx, row.id, [
          ...row.budgetSnapshot.constraints,
          ...fresh.constraints,
          ...(await configuration.budgetConstraints(ctx, row.configSnapshot)),
        ]);
        const observed = input.observed
          ? await options.clock.validateCapture(ctx, input.observed)
          : await options.clock.capture(ctx, {
              operationId: `${row.logicalOperationId}/step${row.physicalOrdinal}/observed`,
              selection: row.preparation
                ? {
                    domainId: row.preparation.referenceTime.point.domainId,
                    ...('clockRef' in row.preparation.referenceTime
                      ? {
                          preferClock: {
                            ...row.preparation.referenceTime.clockRef,
                            revision: undefined,
                          },
                        }
                      : {}),
                  }
                : undefined,
            });
        return invocations.dispatch(ctx, input.invocationId, observed);
      },
      async complete(
        ctx: CallContext,
        input: unknown,
        local?: NativeHostContext,
      ) {
        const request = parse(NativeInvocationCompletionSchema, input);
        const row = await invocations.fact(request.invocationId);
        if (!row.nativeIdentity)
          throw problem(
            'models.native_call_required',
            'validation',
            '该调用由SDK拥有',
          );
        await authorizeNative(ctx, row, 'complete', local);
        if (
          (row.outputRefs.length &&
            request.outputRef &&
            digest(row.outputRefs) !== digest([request.outputRef])) ||
          (row.formedCapture &&
            request.formed &&
            digest(row.formedCapture) !== digest(request.formed))
        )
          throw problem(
            'models.completion_conflict',
            'conflict',
            '原生调用已有不同输出或形成时间',
          );
        const call = await invocations.terminal(ctx, request.invocationId, {
          status: request.status,
          outputRefs: request.outputRef ? [request.outputRef] : [],
          metadata: {
            usage: request.usage,
            rawUsage: request.rawUsage,
            providerRequestId: request.providerRequestId,
          },
        });
        if(call.status==='completed'&&request.outputRef)await invocations.attachOutputs(ctx,request.invocationId,[request.outputRef]);
        if(call.status!=='completed')return call;
        const recordingContext=local?.hostContext??ctx;
        let formed:import('@nous-wave/contracts/time').TimeCaptureReceipt|undefined;
        try{
          formed=request.formed?await options.clock.validateCapture(ctx,request.formed)
            :row.status!=='completed'&&call.observed?await options.clock.captureTerminal(recordingContext,{
              operationId:`${call.identity.logicalOperationId}/step${call.identity.physicalOrdinal}/formed`,
              originRef:{product:options.product,installationId:options.installationId,...(call.subjectId?{subjectId:call.subjectId}:{}),kind:'ModelInvocation',id:call.identity.invocationId},consumer:call.consumer,observed:call.observed,
            }):undefined;
          return await invocations.adopt(ctx,request.invocationId,formed,recordingContext);
        }catch(error){
          if(request.formed)throw error;
          const p=error&&typeof error==='object'&&'problem'in error?error.problem:undefined;
          if(p&&typeof p==='object'&&'category'in p&&['unavailable','forbidden'].includes(String(p.category)))return invocations.deferAdoption(request.invocationId);
          throw error;
        }
      },
    },
    invocations: {
      list: (ctx: CallContext, input?: unknown) => invocations.list(ctx, input),
      read: invocations.read,
    },
    reconcile: invocations.reconcile,
    costs: {
      read: (ctx: CallContext, input?: unknown) =>
        invocations.list(ctx, input, 'costs.read'),
    },
    close() {
      closed = true;
      configuration.close();
    },
  };
  return api;
}
export type Models = ReturnType<typeof createModels>;

export { createModelTest } from './test.js';
