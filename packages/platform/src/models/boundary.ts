// Official Provider models execute only inside the current network/secret callback.
// No SDK model carrying a key is returned or cached by this module.

import type {
  EmbeddingModelV4,
  LanguageModelV4,
  LanguageModelV4GenerateResult,
  LanguageModelV4StreamPart,
  LanguageModelV4StreamResult,
  ProviderV4,
  RerankingModelV4,
  SpeechModelV4,
  TranscriptionModelV4,
} from '@ai-sdk/provider';
import {
  type EmbeddingModelProviderOptionsTransformer,
  EXPERIMENTAL_EMBEDDING_MODEL_MAX_INPUT_BYTES_PER_CALL as maxInputBytes,
  EXPERIMENTAL_EMBEDDING_MODEL_PROVIDER_OPTIONS_TRANSFORMER as optionsTransformer,
} from '@ai-sdk/provider-utils';
import { DBOS } from '@dbos-inc/dbos-sdk';
import { durableCalls, durableEmbeddingCalls } from '@dbos-inc/vercel-ai';
import type { CallContext, Ref } from '@nous-wave/contracts/common';
import {
  type InvocationSnapshot,
  type ModelInvocationIdentity,
  type NormalizedUsage,
  NormalizedUsageSchema,
} from '@nous-wave/contracts/models';
import {
  createProviderRegistry,
  wrapEmbeddingModel,
  wrapLanguageModel,
} from 'ai';
import { problem } from '../errors.js';
import type { NetworkMetadata } from '../network/index.js';
import { digest, immutable } from '../settings/catalog.js';
import { readReal } from '../time/index.js';
import type { ModelConfiguration } from './configuration.js';
import { embeddingPlan } from './embedding-plan.js';
import {
  providerError,
  providerProblem,
  safeProviderStream,
} from './errors.js';
import type { InvocationOwner } from './invocations.js';
import type {
  ModelCallOptions,
  ModelsOptions,
  PreparedModel,
  TerminalMetadata,
} from './types.js';
import { languageUsage } from './usage.js';

type Families = {
  language: LanguageModelV4;
  embedding: EmbeddingModelV4;
  reranking: RerankingModelV4;
  transcription: TranscriptionModelV4;
  speech: SpeechModelV4;
};
type FullEmbedding = EmbeddingModelV4 & {
  [maxInputBytes]?: number | PromiseLike<number | undefined>;
  [optionsTransformer]?: EmbeddingModelProviderOptionsTransformer;
};
export function createModelBoundary(
  options: ModelsOptions,
  configuration: ModelConfiguration,
  invocations: InvocationOwner,
) {
  function checkedMetadata(read: () => TerminalMetadata): TerminalMetadata {
    try {
      const metadata = read();
      if (metadata.usage) NormalizedUsageSchema.parse(metadata.usage);
      return metadata;
    } catch {
      return {
        problem: problem(
          'models.usage_invalid',
          'failed',
          '实际模型已完成，用量格式需要核实',
        ).problem,
      };
    }
  }
  function official<K extends keyof Families>(
    snapshot: InvocationSnapshot,
    kind: K,
    apiKey: string | undefined,
    fetcher: typeof fetch,
  ): Families[K] {
    if (snapshot.definition.managedBy !== 'sdk' || !snapshot.provider)
      throw problem(
        'models.native_binding',
        'validation',
        'SDK调用需要正式SDK资源分支',
      );
    const adapter = options.providerAdapters.find(
      (item) => item.id === snapshot.provider!.adapter,
    );
    if (!adapter)
      throw problem(
        'models.adapter_not_found',
        'unavailable',
        '模型Provider适配器不可用',
      );
    const sdk = adapter.create({
      provider: snapshot.provider,
      apiKey,
      fetch: fetcher,
    });
    const registry = createProviderRegistry({ configured: sdk }),
      id = `configured:${snapshot.definition.definition.modelId}` as const;
    return (
      kind === 'language'
        ? registry.languageModel(id)
        : kind === 'embedding'
          ? registry.embeddingModel(id)
          : kind === 'reranking'
            ? registry.rerankingModel(id)
            : kind === 'transcription'
              ? registry.transcriptionModel(id)
              : registry.speechModel(id)
    ) as Families[K];
  }
  async function metadata<K extends keyof Families, T>(
    snapshot: InvocationSnapshot,
    kind: K,
    use: (model: Families[K]) => T | PromiseLike<T>,
  ): Promise<T> {
    const noFetch: typeof fetch = async () => {
      throw problem(
        'models.metadata_network',
        'forbidden',
        '只读SDK元数据不能执行网络调用',
      );
    };
    return use(official(snapshot, kind, '', noFetch));
  }
  function inputDigest(value: unknown) {
    if (!value || typeof value !== 'object') return digest(value);
    const { abortSignal: _signal, ...actual } = value as Record<
      string,
      unknown
    >;
    return digest(actual);
  }
  async function guard(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    ordinal: number,
    value: unknown,
  ) {
    const row = await options.db
      .selectFrom('modelInvocations')
      .selectAll()
      .where('logicalOperationId', '=', ctx.trace.operationId)
      .where('physicalOrdinal', '=', ordinal)
      .where('attemptNo', '=', 1)
      .executeTakeFirst();
    if (row) {
      if (
        row.subjectId !== (ctx.subjectId ?? null) ||
        digest(row.consumer) !== digest(ctx.consumer) ||
        row.roleId !== prepared.snapshot.roleId ||
        digest(row.configSnapshot.actualResource) !==
          digest(prepared.snapshot.actualResource)
      )
        throw problem(
          'models.invocation_scope',
          'forbidden',
          '实际物理调用的主体、消费者或模型不匹配',
        );
      await options.authorize(ctx, 'models.invocations.read', {
        product: options.product,
        installationId: options.installationId,
        ...(row.subjectId ? { subjectId: row.subjectId } : {}),
        kind: 'ModelInvocation',
        id: row.id,
      });
      await options.data.validateSources(ctx, row.inputSources);
      if (
        row.preparation?.inputDigest &&
        row.preparation.inputDigest !== inputDigest(value)
      )
        throw problem(
          'models.input_changed',
          'conflict',
          '同一物理调用身份的实际输入已变化',
        );
    } else await configuration.assertCurrent(ctx, prepared.snapshot);
    await options.data.validateSources(ctx, request.inputRefs);
    return row;
  }
  async function withProvider<K extends keyof Families, T>(
    ctx: CallContext,
    snapshot: InvocationSnapshot,
    kind: K,
    use: (
      model: Families[K],
      network: NetworkMetadata,
      credential?: { ref: string; revision: number; generation: number },
    ) => Promise<T>,
  ): Promise<T> {
    await configuration.assertCurrent(ctx, snapshot);
    if (snapshot.definition.managedBy !== 'sdk' || !snapshot.provider)
      throw problem(
        'models.native_binding',
        'validation',
        'SDK调用需要正式SDK资源分支',
      );
    const provider = snapshot.provider,
      resource = snapshot.definition.definition;
    const adapter = options.providerAdapters.find(
      (item) => item.id === provider.adapter,
    );
    if (!adapter)
      throw problem(
        'models.adapter_not_found',
        'unavailable',
        '模型Provider适配器不可用',
      );
    const resourceRef = configuration.modelRef(
      resource.id,
      snapshot.actualResource.revision,
    );
    const transport = await options.network.client(ctx, {
      resource: adapter.networkScope({ provider, resourceRef }),
    });
    try {
      const run = async (
        apiKey?: string,
        credential?: { ref: string; revision: number; generation: number },
      ) => {
        if (
          snapshot.credential &&
          digest(snapshot.credential) !== digest(credential)
        )
          throw problem(
            'models.credential_changed',
            'conflict',
            '模型凭据代际已变化，请重新准备调用',
          );
        if (
          snapshot.network &&
          digest(snapshot.network) !==
            digest({
              profileId: transport.metadata.profileId,
              revisions: transport.metadata.revisionRefs,
            })
        )
          throw problem(
            'models.network_changed',
            'conflict',
            '模型网络配置已变化，请重新准备调用',
          );
        const secretUse = {
          ref: provider.credentialRef!,
          purpose: 'model.provider',
          resourceRef: {
            product: options.product,
            installationId: options.installationId,
            kind: 'Provider',
            id: provider.id,
          } as Ref,
          useRef: resourceRef,
        };
        // Input/capture/admission may wait after constructing the SDK object. Recheck
        // the actual Provider generation at its real fetch, while retaining this
        // callback for the complete response/stream lifetime.
        const currentFetch: typeof fetch = provider.credentialRef
          ? async (input, init) =>
              options.credentials.withSecret(
                ctx,
                secretUse,
                async (_bytes, current) => {
                  if (digest(current) !== digest(credential))
                    throw problem(
                      'models.credential_changed',
                      'conflict',
                      '模型凭据在实际派发前已修订或撤销',
                    );
                  return transport.fetch(input, init);
                },
              )
          : transport.fetch;
        const model = official(snapshot, kind, apiKey, currentFetch);
        return use(model as Families[K], transport.metadata, credential);
      };
      return provider.credentialRef
        ? await options.credentials.withSecret(
            ctx,
            {
              ref: provider.credentialRef,
              purpose: 'model.provider',
              resourceRef: {
                product: options.product,
                installationId: options.installationId,
                kind: 'Provider',
                id: provider.id,
              },
              useRef: resourceRef,
            },
            async (bytes, metadata) =>
              run(new TextDecoder().decode(bytes), metadata),
          )
        : await run();
    } finally {
      await transport.close();
    }
  }
  async function freeze(ctx: CallContext, prepared: PreparedModel) {
    if (prepared.snapshot.definition.managedBy === 'native') return prepared;
    return withProvider(
      ctx,
      prepared.snapshot,
      prepared.snapshot.policy.kind,
      async (model, network, credential) =>
        immutable({
          ...prepared,
          snapshot: {
            ...prepared.snapshot,
            actualProvider: model.provider,
            actualModelId: model.modelId,
            ...(credential ? { credential } : {}),
            network: {
              profileId: network.profileId,
              revisions: network.revisionRefs,
            },
          },
        }),
    );
  }
  async function preparePhysical(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    ordinal: number,
    value: unknown,
    stepNo?: number,
    batchNo?: number,
  ) {
    if (request.inputRefs.some((ref) => ref.kind === 'ContextView'))
      throw problem(
        'models.context_as_source',
        'validation',
        'ContextView必须保存为准备元数据',
      );
    const params = value as Record<string, unknown>;
    const inference = {
      ...prepared.snapshot.inference,
      ...Object.fromEntries(
        [
          'maxOutputTokens',
          'temperature',
          'topP',
          'topK',
          'presencePenalty',
          'frequencyPenalty',
          'stopSequences',
          'seed',
          'providerOptions',
          'voice',
          'outputFormat',
          'speed',
          'language',
          'topN',
        ].flatMap((key) =>
          params[key] === undefined ? [] : [[key, params[key]]],
        ),
      ),
    };
    const snapshot = { ...prepared.snapshot, inference } as InvocationSnapshot;
    await configuration.assertCurrent(ctx, snapshot);
    await options.data.validateSources(ctx, request.inputRefs);
    const observed = await options.clock.capture(ctx, {
      operationId: `${ctx.trace.operationId}/step${ordinal}/observed`,
      selection: request.selection??(request.preparation?{domainId:request.preparation.referenceTime.point.domainId,...('clockRef'in request.preparation.referenceTime?{preferClock:{...request.preparation.referenceTime.clockRef,revision:undefined}}:{})}:undefined),
    });
    const { captureRef: _capture, ...referenceTime } = observed;
    const preparation = {
      ...(request.preparation ?? { referenceTime }),
      inputDigest: inputDigest(value),
    };
    const estimate = request.estimate ?? {
      value: Buffer.byteLength(JSON.stringify(value)),
      method: 'utf8-bytes-upper-bound',
    };
    const identity = {
      logicalOperationId: ctx.trace.operationId,
      physicalOrdinal: ordinal,
      attemptNo: 1,
      ...(stepNo === undefined ? {} : { stepNo }),
      ...(batchNo === undefined ? {} : { batchNo }),
      ...(DBOS.isWithinWorkflow() ? { workflowId: DBOS.workflowID } : {}),
    };
    const current = await invocations.prepare(ctx, {
      identity,
      snapshot,
      constraints: prepared.constraints,
      inputRefs: request.inputRefs,
      estimate,
      maxOutputTokens:
        snapshot.policy.kind === 'language'
          ? typeof inference.maxOutputTokens === 'number'
            ? inference.maxOutputTokens
            : snapshot.policy.inference.maxOutputTokens
          : 0,
      observed,
      preparation,
    });
    if (current.status === 'completed') return { call: current, replay: true };
    if (current.status !== 'prepared')
      throw problem(
        'models.effect_unknown',
        'effect_unknown',
        '实际模型调用已派发，需核实原结果',
        { operationId: ctx.trace.operationId, retry: { mode: 'reconcile' } },
      );
    const inputRefs = await options.data.saveInput(ctx, {
      identity: current.identity,
      snapshot,
      value,
      inputRefs: request.inputRefs,
      observed,
      preparation,
    });
    // Input persistence precedes dispatch. It is not an Encounter until actual consumption.
    await options.db
      .updateTable('modelInvocations')
      .set({ semanticInputRefs: JSON.stringify(inputRefs) })
      .where('id', '=', current.identity.invocationId)
      .execute();
    const fresh = await options.budgetScopes.resolve(ctx, {
      roleId: snapshot.roleId,
      allocation: request.budgetAllocation,
    });
    if (
      snapshot.budgetAllocation &&
      digest(fresh.allocation) !== digest(snapshot.budgetAllocation)
    )
      throw problem(
        'models.allocation_changed',
        'forbidden',
        '根Task子额度当前状态已变化',
      );
    await invocations.recheck(ctx, current.identity.invocationId, [
      ...prepared.constraints,
      ...fresh.constraints,
      ...(await configuration.budgetConstraints(ctx, snapshot)),
    ]);
    return {
      call: await invocations.dispatch(
        ctx,
        current.identity.invocationId,
        observed,
      ),
      replay: false,
    };
  }
  async function result(
    ctx: CallContext,
    call: Awaited<ReturnType<typeof preparePhysical>>['call'],
    value: unknown,
    metadata: TerminalMetadata,
  ) {
    const realAt = readReal().at;
    if(!call.observed)throw problem('models.observed_missing','conflict','实际模型结果缺少原观察回执');
    const formed = options.clock.captureTerminal(ctx,{
      operationId:`${call.identity.logicalOperationId}/step${call.identity.physicalOrdinal}/formed`,
      originRef:{product:options.product,installationId:options.installationId,...(call.subjectId?{subjectId:call.subjectId}:{}),kind:'ModelInvocation',id:call.identity.invocationId},
      consumer:call.consumer,observed:call.observed,
    })
      .then(
        (receipt) => ({ receipt }),
        (error) => ({ error }),
      );
    // Known Provider completion is a durable fact even if storage/adoption/capture fails afterwards.
    await invocations.terminal(ctx, call.identity.invocationId, {
      status: 'completed',
      metadata,
      realAt,
    });
    const refs = await options.data.saveOutput(ctx, {
      identity: call.identity,
      snapshot: call.snapshot,
      value,
      inputRefs: call.inputRefs,
      realAt,
    });
    await invocations.attachOutputs(ctx, call.identity.invocationId, refs);
    const captured = await formed;
    if ('error' in captured) throw providerError(captured.error);
    await invocations.adopt(ctx, call.identity.invocationId, captured.receipt);
  }
  async function failed(ctx: CallContext, id: string, error: unknown) {
    const row = await invocations.fact(id);
    if (row.status !== 'completed')
      await invocations.terminal(ctx, id, {
        status: 'unknown',
        metadata: { problem: providerProblem(error) },
      });
    options.diagnostic?.({...ctx,trace:{...ctx.trace,invocationId:id}}, providerProblem(error));
  }
  async function run<K extends keyof Families, T>(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    ordinal: number,
    kind: K,
    value: unknown,
    execute: (model: Families[K]) => PromiseLike<T>,
    metadata: (value: T) => TerminalMetadata,
    stepNo?: number,
    batchNo?: number,
  ): Promise<T> {
    const previous = await guard(ctx, prepared, request, ordinal, value);
    if (previous?.status === 'completed') {
      await invocations.adopt(ctx, previous.id);
      return options.data.loadOutput(ctx, {
        identity: invocations.view(previous).identity,
        outputRefs: previous.outputRefs,
      }) as Promise<T>;
    }
    if (previous && previous.status !== 'prepared')
      throw problem(
        'models.effect_unknown',
        'effect_unknown',
        '实际调用已派发，需要核实原结果',
        { retry: { mode: 'reconcile' } },
      );
    return withProvider(ctx, prepared.snapshot, kind, async (model) => {
      const physical = await preparePhysical(
        ctx,
        prepared,
        request,
        ordinal,
        value,
        stepNo,
        batchNo,
      );
      if (physical.replay) {
        await invocations.adopt(ctx, physical.call.identity.invocationId);
        return options.data.loadOutput(ctx, {
          identity: physical.call.identity,
          outputRefs: physical.call.outputRefs,
        }) as Promise<T>;
      }
      try {
        const value = await execute(model);
        const normalized = checkedMetadata(() => ({
          ...metadata(value),
          ...(prepared.snapshot.provider
            ? options.providerAdapters
                .find(
                  (adapter) =>
                    adapter.id === prepared.snapshot.provider!.adapter,
                )
                ?.usage?.(kind, value)
            : undefined),
        }));
        await result(ctx, physical.call, value, normalized);
        return value;
      } catch (error) {
        await failed(ctx, physical.call.identity.invocationId, error);
        throw providerError(error);
      }
    });
  }
  async function language(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    step: () => number,
  ): Promise<LanguageModelV4> {
    const actual = await metadata(
      prepared.snapshot,
      'language',
      async (model) => ({
        provider: model.provider,
        modelId: model.modelId,
        supportedUrls: await model.supportedUrls,
      }),
    );
    const safe: LanguageModelV4 = {
      specificationVersion: 'v4',
      ...actual,
      doGenerate(params) {
        const ordinal = step();
        return run(
          ctx,
          prepared,
          request,
          ordinal,
          'language',
          params,
          (model) => model.doGenerate(params),
          (raw) => ({
            usage: languageUsage(raw.usage),
            rawUsage: raw.usage,
            providerRequestId: raw.response?.id,
          }),
          ordinal,
        );
      },
      async doStream(params) {
        const ordinal = step();
        const ready = Promise.withResolvers<LanguageModelV4StreamResult>();
        const lifetime = (async () => {
          const previous = await guard(ctx, prepared, request, ordinal, params);
          if (previous?.status === 'completed') {
            await invocations.adopt(ctx, previous.id);
            const saved = (await options.data.loadOutput(ctx, {
              identity: invocations.view(previous).identity,
              outputRefs: previous.outputRefs,
            })) as { parts: LanguageModelV4StreamPart[] };
            ready.resolve({
              stream: new ReadableStream({
                start(controller) {
                  for (const part of saved.parts) controller.enqueue(part);
                  controller.close();
                },
              }),
            });
            return;
          }
          if (previous && previous.status !== 'prepared')
            throw problem(
              'models.effect_unknown',
              'effect_unknown',
              '实际调用已派发，需要核实原结果',
              { retry: { mode: 'reconcile' } },
            );
          return withProvider(
            ctx,
            prepared.snapshot,
            'language',
            async (model) => {
              const physical = await preparePhysical(
                ctx,
                prepared,
                request,
                ordinal,
                params,
                ordinal,
              );
              if (physical.replay) {
                await invocations.adopt(
                  ctx,
                  physical.call.identity.invocationId,
                );
                const saved = (await options.data.loadOutput(ctx, {
                  identity: physical.call.identity,
                  outputRefs: physical.call.outputRefs,
                })) as { parts: LanguageModelV4StreamPart[] };
                ready.resolve({
                  stream: new ReadableStream({
                    start(controller) {
                      for (const part of saved.parts) controller.enqueue(part);
                      controller.close();
                    },
                  }),
                });
                return;
              }
              const done = Promise.withResolvers<void>();
              let settled = false;
              const parts: LanguageModelV4StreamPart[] = [];
              try {
                const raw = await model.doStream(params),
                  reader = safeProviderStream(raw.stream).getReader();
                let terminal = false;
                ready.resolve({
                  ...raw,
                  stream: new ReadableStream<LanguageModelV4StreamPart>({
                    async pull(controller) {
                      try {
                        const next = await reader.read();
                        if (next.done) {
                          if (!terminal)
                            throw problem(
                              'models.stream_incomplete',
                              'effect_unknown',
                              '模型流结束但没有完成事实',
                            );
                          settled = true;
                          done.resolve();
                          reader.releaseLock();
                          controller.close();
                          return;
                        }
                        const part = next.value;
                        parts.push(part);
                        if (part.type === 'error') throw part.error;
                        if (part.type === 'finish') {
                          terminal = true;
                          const response = parts.find(
                            (part) => part.type === 'response-metadata',
                          );
                          await result(
                            ctx,
                            physical.call,
                            { parts },
                            checkedMetadata(() => ({
                              usage: languageUsage(part.usage),
                              rawUsage: part.usage,
                              providerRequestId:
                                response?.type === 'response-metadata'
                                  ? response.id
                                  : undefined,
                            })),
                          );
                        }
                        controller.enqueue(part);
                      } catch (error) {
                        await failed(
                          ctx,
                          physical.call.identity.invocationId,
                          error,
                        );
                        settled = true;
                        try {
                          await reader.cancel('model-failed');
                        } catch {}
                        done.resolve();
                        controller.error(providerError(error));
                      }
                    },
                    async cancel() {
                      try {
                        await reader.cancel('cancelled');
                        if (!terminal)
                          await failed(
                            ctx,
                            physical.call.identity.invocationId,
                            problem(
                              'models.cancelled',
                              'effect_unknown',
                              '模型调用已取消，费用待核实',
                            ),
                          );
                      } catch (error) {
                        throw providerError(error);
                      } finally {
                        settled = true;
                        done.resolve();
                      }
                    },
                  }),
                });
                await done.promise;
              } catch (error) {
                await failed(ctx, physical.call.identity.invocationId, error);
                ready.reject(providerError(error));
                if (!settled) done.resolve();
              }
            },
          );
        })();
        void lifetime.catch((error) => ready.reject(providerError(error)));
        return ready.promise;
      },
    };
    return wrapLanguageModel({
      model: safe,
      middleware: [
        {
          specificationVersion: 'v4',
          wrapGenerate: async ({ doGenerate, params }) => {
            await guard(ctx, prepared, request, step(), params);
            return doGenerate();
          },
          wrapStream: async ({ doStream, params }) => {
            await guard(ctx, prepared, request, step(), params);
            return doStream();
          },
        },
        durableCalls({
          retriesAllowed: false,
          include: { requestBody: false, responseBody: false },
        }),
      ],
    });
  }
  const embeddingBatch = DBOS.registerWorkflow(
    async (
      ctx: CallContext,
      prepared: PreparedModel,
      request: ModelCallOptions,
      batch: number,
      params: Omit<Parameters<EmbeddingModelV4['doEmbed']>[0], 'abortSignal'>,
    ) => {
      const actual = await metadata(
        prepared.snapshot,
        'embedding',
        async (model) => ({
          provider: model.provider,
          modelId: model.modelId,
          maxEmbeddingsPerCall: await model.maxEmbeddingsPerCall,
          supportsParallelCalls: await model.supportsParallelCalls,
        }),
      );
      const safe: EmbeddingModelV4 = {
        specificationVersion: 'v4',
        ...actual,
        doEmbed(value) {
          return run(
            ctx,
            prepared,
            request,
            batch,
            'embedding',
            value,
            (model) => model.doEmbed(value),
            (raw) => ({
              usage: raw.usage
                ? { inputTokens: raw.usage.tokens, outputTokens: 0 }
                : undefined,
              rawUsage: raw.usage,
            }),
            undefined,
            batch,
          );
        },
      };
      const durable = wrapEmbeddingModel({
        model: safe,
        middleware: [
          {
            specificationVersion: 'v4',
            wrapEmbed: async ({ doEmbed, params }) => {
              await guard(ctx, prepared, request, batch, params);
              return doEmbed();
            },
          },
          durableEmbeddingCalls({ retriesAllowed: false }),
        ],
      });
      return durable.doEmbed(params);
    },
    {
      name: `${options.product}.${options.installationId}.models.embeddingBatch`,
    },
  );
  async function embedding(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    values: readonly string[],
  ): Promise<EmbeddingModelV4> {
    const actual = await metadata(
      prepared.snapshot,
      'embedding',
      async (model) => ({
        provider: model.provider,
        modelId: model.modelId,
        maxEmbeddingsPerCall: await model.maxEmbeddingsPerCall,
        supportsParallelCalls: await model.supportsParallelCalls,
        [maxInputBytes]: await (model as FullEmbedding)[maxInputBytes],
        hasTransformer: !!(model as FullEmbedding)[optionsTransformer],
      }),
    );
    let ordinal = 0;
    // This freezes the SDK's existing batch split, without replacing its dispatch/parallel loop.
    const plan = embeddingPlan(
      values,
      actual.maxEmbeddingsPerCall ?? Infinity,
      actual[maxInputBytes] ?? Infinity,
    );
    const model: FullEmbedding = {
      specificationVersion: 'v4',
      ...actual,
      ...(actual.hasTransformer
        ? {
            [optionsTransformer]: (
              input: Parameters<EmbeddingModelProviderOptionsTransformer>[0],
            ) =>
              metadata(prepared.snapshot, 'embedding', async (real) =>
                (real as FullEmbedding)[optionsTransformer]!(input),
              ),
          }
        : {}),
      async doEmbed(params) {
        const batch = ordinal++;
        if (digest(plan[batch]) !== digest(params.values))
          throw problem(
            'models.embedding_plan_changed',
            'conflict',
            'SDK实际向量批次与冻结计划不一致',
          );
        const safe: EmbeddingModelV4 = {
          specificationVersion: 'v4',
          ...actual,
          doEmbed(value) {
            return run(
              ctx,
              prepared,
              request,
              batch,
              'embedding',
              value,
              (model) => model.doEmbed(value),
              (raw) => ({
                usage: raw.usage
                  ? { inputTokens: raw.usage.tokens, outputTokens: 0 }
                  : undefined,
                rawUsage: raw.usage,
              }),
              undefined,
              batch,
            );
          },
        };
        if (!DBOS.isWithinWorkflow() || DBOS.isInStep())
          return wrapEmbeddingModel({
            model: safe,
            middleware: [
              {
                specificationVersion: 'v4',
                wrapEmbed: async ({ doEmbed, params }) => {
                  await guard(ctx, prepared, request, batch, params);
                  return doEmbed();
                },
              },
              durableEmbeddingCalls({ retriesAllowed: false }),
            ],
          }).doEmbed(params);
        const { abortSignal, ...saved } = params;
        const childId = `models-embedding-${digest([ctx.trace.operationId, prepared.snapshot.config.digest, batch])}`;
        const child = await DBOS.startWorkflow(embeddingBatch, {
          workflowID: childId,
          workflowAttributes: {
            ...(ctx.subjectId ? { subjectId: ctx.subjectId } : {}),
            traceId: ctx.trace.traceId,
            operation: 'models.embeddingBatch',
          },
        })(
          ctx,
          prepared,
          {
            roleId: request.roleId,
            inputRefs: request.inputRefs,
            ...(request.estimate ? { estimate: request.estimate } : {}),
            ...(request.selection ? { selection: request.selection } : {}),
            ...(request.budgetAllocation
              ? { budgetAllocation: request.budgetAllocation }
              : {}),
          },
          batch,
          saved,
        );
        const cancel = () => {
          void DBOS.cancelWorkflow(childId);
        };
        abortSignal?.addEventListener('abort', cancel, { once: true });
        if (abortSignal?.aborted) cancel();
        try {
          return await child.getResult();
        } finally {
          abortSignal?.removeEventListener('abort', cancel);
        }
      },
    };
    return model;
  }
  function family<K extends 'reranking' | 'transcription' | 'speech'>(
    ctx: CallContext,
    prepared: PreparedModel,
    request: ModelCallOptions,
    kind: K,
  ) {
    const invoke = async <T>(
      value: unknown,
      execute: (model: Families[K]) => PromiseLike<T>,
      metadata: (value: T) => TerminalMetadata,
    ) => {
      await guard(ctx, prepared, request, 0, value);
      const call = () =>
        run(ctx, prepared, request, 0, kind, value, execute, metadata);
      return DBOS.isWithinWorkflow() && !DBOS.isInStep()
        ? DBOS.runStep(call, { name: `models.${kind}`, retriesAllowed: false })
        : call();
    };
    return {
      invoke,
      metadata: () =>
        metadata(prepared.snapshot, kind, async (model) => ({
          provider: model.provider,
          modelId: model.modelId,
        })),
    };
  }
  return { freeze, withProvider, language, embedding, family };
}
