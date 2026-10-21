import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { DBOS, type WorkflowHandle } from '@dbos-inc/dbos-sdk';
import {
  type CallContext,
  type Policy,
  type Ref,
  type TimePoint,
  TimePointInputSchema,
} from '@nous-wave/contracts/common';
import { KnowledgeKindSchema } from '@nous-wave/contracts/knowledge';
import type { BlobMetadata, BlobResult } from '@nous-wave/contracts/materials';
import type {
  HostStatus,
  OperationView,
  OperationTrace,
} from '@nous-wave/contracts/runtime';
import type { TimeRecordings } from '@nous-wave/contracts/time';
import type { BootstrapView } from '@nous-wave/platform/bootstrap';
import {
  prepareDirectories,
  withBootstrapDatabase,
} from '@nous-wave/platform/bootstrap';
import type { SecretRegistry } from '@nous-wave/platform/credentials';
import {
  type Authentication,
  createAuthentication,
  createBootstrapSecrets,
  createSecretRegistry,
} from '@nous-wave/platform/credentials';
import { problem, safeProblem } from '@nous-wave/platform/errors';
import {
  createModels,
  createModelTest,
  type Models,
  type ModelsOptions,
  modelSettingsModules,
  officialProviderAdapters,
} from '@nous-wave/platform/models';
import {
  createNetwork,
  networkSettingsModule,
} from '@nous-wave/platform/network';
import { createPrompts, promptsDefinition,createPromptDataLifecycle } from '@nous-wave/platform/prompts';
import {
  dbosSystemDatabaseURL,
  describeDurableWorkflow,
  describeDurableSteps,
  resumeDurableWorkflow,
  registerProblemSerialization,
} from '@nous-wave/platform/storage';
import {
  createTelemetry,
  type Telemetry,
  telemetryDefinition,
} from '@nous-wave/platform/telemetry';
import { canonicalize } from 'json-canonicalize';
import type {Kysely} from 'kysely';
import { createCognition } from './cognition/index.js';
import {createData,dataSettingsModule,type Data} from './data/index.js';
import {createIdentityData} from './identity/data.js';
import {createMaterialData} from './materials/data.js';
import {createKnowledgeData} from './knowledge/data.js';
import {createCognitionData} from './cognition/data.js';
import {createResourceData} from './resources/data.js';
import { createIdentity } from './identity/index.js';
import { createKnowledge } from './knowledge/index.js';
import { createOrganization } from './knowledge/organization.js';
import { createSourceDocuments } from './knowledge/source-documents.js';
import { createProductSettings } from './management/settings.js';
import { createMaterials } from './materials/index.js';
import { promptCatalog, roleCatalog } from './models/catalog.js';
import { createModelData } from './models/data.js';
import {
  createNativeHost,
  type NativeHost,
  nativeSettingsModule,
} from './models/native.js';
import {
  issuancePolicy,
  managementScope,
  operationPolicy,
  subjectOperations,
} from './platform/authorization.js';
import { makeContext, now, type Storage } from './platform/context.js';
import { openDatabase } from './platform/database.js';
import { fail } from './platform/errors.js';
import { requireSubject } from './platform/security.js';
import { createResources } from './resources/index.js';
import { createRetrieval } from './retrieval/index.js';
import {
  type ClockSource,
  createTime,
  timeSettingsModule,
} from './time/index.js';

export interface NousOptions {
  bootstrap: BootstrapView;
  databaseUrl: string;
  authSecret: string;
  applicationVersion?: string;
  mode?: 'service' | 'cli';
  budgetScopes?: ModelsOptions['budgetScopes'];
  sourcePolicies?: (ctx: CallContext, roots: Ref[]) => Promise<Policy[]>;
  backgroundMaintenance?: boolean;
  clockSources?: Readonly<Record<string, ClockSource>>;
  embedding?: {
    id: string;
    dimensions: number;
    embed(ctx: CallContext, texts: string[]): Promise<number[][]>;
  };
}
type Cognition = ReturnType<typeof createCognition>;
type Organization = ReturnType<typeof createOrganization>;
type Durable<F extends (...args: any[]) => Promise<any>> = F & {
  start(
    ...args: Parameters<F>
  ): Promise<WorkflowHandle<Awaited<ReturnType<F>>>>;
};
/** Product composition exposes owner operations; it does not expand their internal representations. */
export interface Nous {
  installationId: string;
  subjects: ReturnType<typeof createIdentity>;
  telemetry: Telemetry;
  host: {
    start(): Promise<void>;
    drain(): Promise<{ state: string; acceptingWork: boolean }>;
    status(): Promise<HostStatus>;
    doctor(): Promise<{
      product: string;
      installationId: string;
      database: string;
      runtime: string;
      time: TimePoint;
    }>;
    stop(mode?: 'graceful' | 'force'): Promise<void>;
  };
  time: ReturnType<typeof createTime>;
  materials: ReturnType<typeof createMaterials>['materials'];
  encounters: ReturnType<typeof createMaterials>['encounters'];
  knowledge: ReturnType<typeof createKnowledge> & {
    query: ReturnType<typeof createRetrieval>['query'];
    expand: ReturnType<typeof createRetrieval>['expand'];
    explainQuery: ReturnType<typeof createRetrieval>['explain'];
  };
  contexts: Cognition['contexts'];
  growth: Cognition['growth'] & {
    run: Durable<Cognition['growth']['run']>;
    apply: Durable<Cognition['growth']['apply']>;
    resolve: Durable<Cognition['growth']['resolve']>;
  };
  seeds: Cognition['seeds'] & {
    preview: Durable<Cognition['seeds']['preview']>;
    apply: Durable<Cognition['seeds']['apply']>;
  };
  maintenance: Cognition['maintenance'] & {
    run: Durable<Cognition['maintenance']['run']>;
  };
  feedback: Cognition['feedback'];
  episodes: Organization['episodes'];
  journals: { write: Durable<Organization['journals']['write']> };
  concepts: Organization['concepts'];
  motivation: Organization['motivation'];
  persona: Organization['persona'];
  resources: ReturnType<typeof createResources> & {
    read: Durable<ReturnType<typeof createResources>['read']>;
  };
  settings: ReturnType<typeof createProductSettings>;
  secrets: SecretRegistry;
  prompts: ReturnType<typeof createPrompts>;
  data:Data&{export:Durable<Data['export']>;import:Durable<Data['import']>;purge:Durable<Data['purge']>};
  network: ReturnType<typeof createNetwork>;
  testModel: ReturnType<typeof createModelTest>;
  models: Models;
  nativeModels: NativeHost['facade'];
  operations: {
    inspect(ctx: CallContext, id: string): Promise<OperationView>;
    trace(ctx: CallContext, id: string): Promise<OperationTrace>;
    cancel(ctx: CallContext, id: string): Promise<OperationView>;
    resume(ctx: CallContext, id: string): Promise<OperationView>;
  };
  authentication: Authentication;
  authorize: Storage['authorize'];
  managementScope: () => ReturnType<typeof managementScope>;
  credentials: {
    delegations: Authentication['delegations'];
    createManagementKey(
      ctx: CallContext,
      name: string,
    ): ReturnType<Authentication['createManagementKey']>;
  };
}
export async function createNous(options: NousOptions): Promise<Nous> {
  const { paths, bootstrap } = options.bootstrap;
  const installationId = bootstrap.installationId;
  await prepareDirectories(paths);
  const telemetry: Telemetry = createTelemetry({
    product: 'nous',
    installationId,
    paths,
    mode: options.mode ?? 'service',
    bootstrapLevel: bootstrap.logging.level,
    events: [
      'http.operation_completed',
      'http.failed',
      'host.started',
      'models.failed',
    ],
  });
  const lifetime = new AsyncDisposableStack();
  lifetime.defer(() => telemetry.close());
  try {
    const version = (
      createRequire(import.meta.url)('@nous-wave/nous/package.json') as {
        version: string;
      }
    ).version;
    DBOS.setConfig({
      name: 'nous-wave',
      systemDatabaseUrl: dbosSystemDatabaseURL(options.databaseUrl),
      applicationVersion: options.applicationVersion ?? version,
      enableOTLP: false,
      logger: telemetry.dbosLogger,
    });
    registerProblemSerialization();
    const database = await openDatabase(
      options.databaseUrl,
      bootstrap.database.maxConnections,
    );
    lifetime.defer(() => database.close());
    const baseURL =
      bootstrap.listen.publicBaseURL ??
      `http://${bootstrap.listen.host}:${bootstrap.listen.port}`;
    const sourceGuard:NonNullable<import('@nous-wave/platform/credentials').AuthenticationOptions['sourceGuard']>=async(ctx,operation,target,client)=>{
      if(['subjects.create','data.import','data.purge','operations.read','operations.cancel'].includes(operation))return;
      const targets=target?('product'in target?[target]:target.kind==='object'?target.refs:target.kind==='workspace'?[target.workspaceRef]:[]):[];
      const ids=new Set([ctx.subjectId,...targets.map(value=>value.subjectId)].filter((value):value is string=>Boolean(value)));
      const db:Kysely<any>=client??database.db;
      for(const id of ids){let query=db.selectFrom('subjects').select('id').where('id','=',id);if(client?.isTransaction)query=query.forKeyShare();if(!await query.executeTakeFirst())throw problem('subjects.not_found','not_found','主体不存在');}
    };
    const authentication = await createAuthentication({
      pool: database.pool,
      db: database.db,
      product: 'nous',
      installationId,
      secret: options.authSecret,
      baseURL,
      trustedOrigins: bootstrap.listen.trustedOrigins,
      operationPolicy,
      issuancePolicy: issuancePolicy(installationId),
      sourceGuard,
    });
    const authorize: Storage['authorize'] = (ctx, operation, target, db) => authentication.authorize(ctx,operation,target,db);
    const store: Storage = {
      authorize,
      installationId,
      get db() {
        return DBOS.isInTransaction()
          ? database.datasource.client
          : database.db;
      },
      transaction: async (fn,options) => {
        if (DBOS.isInTransaction()) return fn(database.datasource.client);
        if (DBOS.isInWorkflow())
          return database.datasource.runTransaction(
            () => fn(database.datasource.client),options,
          );
        const tx=database.db.transaction();
        return (options?.isolationLevel?tx.setIsolationLevel(options.isolationLevel):tx).execute(fn);
      },
    };
    const owner = <T extends object>(extra: T): Storage & T =>
      Object.assign(Object.create(store), extra);
    const subjects = createIdentity(store);
    let knowledge: ReturnType<typeof createKnowledge>;
    const { materials, encounters, sourceRepository, recordings,artifactBlobs } =
      createMaterials(
        owner({
          blobPath: paths.blob,
          readableSources: (ctx: CallContext, refs: Ref[]) =>
            knowledge.sources.readable(ctx, refs),
          resolveSources: (ctx: CallContext, refs: Ref[]) =>
            knowledge.sources.resolve(ctx, refs),
          inheritSources: (ctx: CallContext, refs: Ref[]) =>
            knowledge.sources.inherit(ctx, refs),
          externalPolicies: (ctx: CallContext, refs: Ref[]) =>
            knowledge.sources.externalPolicies(ctx, refs),
        }),
      );
    knowledge = createKnowledge(
      owner({ sourceRepository, sourcePolicies: options.sourcePolicies }),
    );
    const settings = createProductSettings(store, {
      extraModules: [
        telemetryDefinition,
        dataSettingsModule,
        timeSettingsModule,
        ...modelSettingsModules(roleCatalog, officialProviderAdapters).modules,
        promptsDefinition,
        networkSettingsModule,
        nativeSettingsModule,
      ],
    });
    await settings.registerDefinitions();
    await telemetry.bindSettings(settings);
    const settingsListener = await settings.listen(database.createSession);
    lifetime.defer(() => settingsListener.close());
    let timeRecordings!: TimeRecordings;
    const time = createTime(store, {
      settings,
      processBootId: telemetry.processBootId,
      sources: options.clockSources,
      recordings: (capability) => {
        timeRecordings = capability;
      },
    });
    lifetime.defer(() => time.close());
    let models: Models;
    let nativeHost: NativeHost;
    let network: ReturnType<typeof createNetwork>;
    const secrets = createSecretRegistry({
      db: database.db,
      installationId,
      product: 'nous',
      secretRoot: paths.secret,
      authorize,
      purposes: {
        'native.gateway': {
          operation: 'native.connect',
          resourceKinds: ['NativeRuntime'],
          resolveUse: (ctx, input) => nativeHost.credentialTarget(ctx, input),
        },
        'model.provider': {
          operation: 'models.invoke',
          resourceKinds: ['Provider'],
          resolveUse: (ctx, input) => models.credentialTarget(ctx, input),
        },
        'network.ca': {
          operation: 'network.manage',
          resourceKinds: ['NetworkProfile'],
          resolveUse: (ctx, input) => network.credentialTarget(ctx, input),
        },
        'network.client_certificate': {
          operation: 'network.manage',
          resourceKinds: ['NetworkProfile'],
          resolveUse: (ctx, input) => network.credentialTarget(ctx, input),
        },
        'network.client_key': {
          operation: 'network.manage',
          resourceKinds: ['NetworkProfile'],
          resolveUse: (ctx, input) => network.credentialTarget(ctx, input),
        },
        'network.proxy': {
          operation: 'network.manage',
          resourceKinds: ['NetworkProfile'],
          resolveUse: (ctx, input) => network.credentialTarget(ctx, input),
        },
      },
    });
    network = createNetwork({
      product: 'nous',
      installationId,
      processBootId: telemetry.processBootId,
      settings,
      authorize,
      withMaterial: secrets.withSecret,
      resolveResourceNetwork: (ctx, ref) =>
        ref.kind === 'NativeRuntime'
          ? nativeHost.networkSelection(ctx, ref)
          : models.networkSelection(ctx, ref),
    });
    lifetime.defer(() => network.close());
    const prompts = createPrompts({
      installationId,
      db: database.db,
      settings,
      processBootId: telemetry.processBootId,
      programRoot: dirname(
        createRequire(import.meta.url).resolve('@nous-wave/nous/package.json'),
      ),
      overrideRoot: join(paths.config, 'prompts'),
      catalog: promptCatalog,
      authorize: (ctx, input) =>
        authorize(
          ctx,
          `prompts.${input.action}`,
          input.scope.kind === 'subject'
            ? {
                product: 'nous',
                installationId,
                subjectId: input.scope.subjectId,
                kind: 'Subject',
                id: input.scope.subjectId,
              }
            : undefined,
        ),
    });
    lifetime.defer(() => prompts.close());
    await prompts.initialize();
    let cognition: ReturnType<typeof createCognition>;
    const modelData = createModelData({
      installationId,
      db: database.db,
      blobRoot: paths.blob,
      authorize,
      materials,
      recordings,
      validateSources: async (ctx, refs) => {
        if (!refs.length) return;
        if (!ctx.subjectId)
          fail('invalid_input', '安装级模型调用不能读取主体来源');
        await knowledge.sources.resolve(ctx, [...refs]);
      },
    });
    nativeHost = createNativeHost({
      installationId,
      processBootId: telemetry.processBootId,
      settings,
      secrets,
      network,
      authentication,
      authorize,
      models: () => models,
    });
    lifetime.defer(() => nativeHost.close());
    globalThis.AI_SDK_LOG_WARNINGS = false;
    models = createModels({
      product: 'nous',
      installationId,
      processBootId: telemetry.processBootId,
      db: database.db,
      settings,
      credentials: secrets,
      network,
      prompts,
      roleCatalog,
      providerAdapters: officialProviderAdapters,
      clock: { ...time, captureTerminal: timeRecordings.captureTerminal },
      authorize,
      data: modelData,
      budgetScopes: options.budgetScopes ?? {
        resolve: async (_ctx, input) => {
          if (input.allocation)
            fail('capability_unavailable', '本安装未配置根Task额度验证者');
          return { constraints: [] };
        },
      },
      beforeDispatch: assertDispatch,
      diagnostic: (ctx, problem) =>
        telemetry
          .forContext(ctx, 'models')
          .warn({ event: 'models.failed', problem }),
      nativeBindings: nativeHost.bindings,
      contextHooks: {
        async prepare(ctx, input) {
          if (!ctx.subjectId) return { inputRefs: [] };
          if (input.previousPreparation?.contextRef) {
            const view = await cognition.contexts.readView(ctx, {
              id: input.previousPreparation.contextRef.id,
            });
            return {
              instructions: view.blocks.map((block) => block.text).join('\n\n'),
              inputRefs: view.inputPortions.map((portion) => ({
                ...portion.ref,
                selector: portion.selector ?? portion.ref.selector,
              })),
              contextRef: view.ref,
            };
          }
          const material = await materials.capture(
            {
              ...ctx,
              trace: {
                ...ctx.trace,
                operationId: `${ctx.trace.operationId}/step${input.stepNumber}/context-input`,
              },
            },
            {
              kind: 'model_context_input',
              mediaType: 'application/json',
              text: JSON.stringify(input.messages),
              source: { inputs: [...input.inputRefs], roots: [] },
            },
          );
          const resource =
            input.snapshot.definition.managedBy === 'sdk'
              ? input.snapshot.definition.definition
              : input.snapshot.definition.binding;
          if (!resource.contextLimit)
            fail('capability_unavailable', '实际模型缺少上下文窗口元数据');
          const view = await cognition.contexts.prepare(ctx, {
            consumer: ctx.consumer,
            role: input.snapshot.roleId,
            roleTask: '',
            latestMaterials: [
              { ref: material.ref, text: material.text ?? '', required: true },
            ],
            model: {
              resourceId: input.snapshot.actualResource.id,
              modelId: input.snapshot.actualModelId,
              contextLimit: resource.contextLimit,
            },
            budget: {
              outputTokens:
                typeof input.snapshot.inference.maxOutputTokens === 'number'
                  ? input.snapshot.inference.maxOutputTokens
                  : 0,
              requiredTokens: Array.from(JSON.stringify(input.baseInstructions))
                .length,
              toolTokens: Array.from(JSON.stringify(input.tools)).length,
            },
            observed: input.referenceTime.point,
          });
          return {
            instructions: view.blocks.map((block) => block.text).join('\n\n'),
            inputRefs: view.inputPortions.map((portion) => ({
              ...portion.ref,
              selector: portion.selector ?? portion.ref.selector,
            })),
            contextRef: view.ref,
          };
        },
      },
    });
    lifetime.defer(() => models.close());
    const testModel = createModelTest({
      product: 'nous',
      installationId,
      models,
      authorize,
      beforeDispatch: assertDispatch,
    });
    const retrieval = createRetrieval({
      installationId,
      indexPath: join(paths.cache, 'retrieval'),
      knowledge,
      materials,
      capabilities: async (ctx: CallContext) =>
        KnowledgeKindSchema.array().parse(
          (await subjects.read(ctx)).capabilities,
        ),
      encounterInputs: encounters.inputs,
      authorize,
      embed: options.embedding,
      sourceTimes: materials.sourceTimes,
      relations: async (ctx, refs, limit, cutoff) => {
        const relations = await knowledge.relations(
          ctx,
          refs.map((r) => r.id),
          { ...cutoff, limit: Math.min(limit, 200) },
        );
        return relations.items.map((r) => ({
          from: {
            product: 'nous' as const,
            installationId,
            subjectId: ctx.subjectId,
            kind: 'Knowledge',
            id: r.fromId,
          },
          to: {
            product: 'nous' as const,
            installationId,
            subjectId: ctx.subjectId,
            kind: 'Knowledge',
            id: r.toId,
          },
          relation: r.relation,
          weight: r.weight,
        }));
      },
    });
    lifetime.defer(() => retrieval.close());
    const operations = {
      async inspect(ctx: CallContext, id: string) {
        await authorize(ctx, 'operations.read');
        const [status] = await DBOS.listWorkflows({
          workflowIDs: [id],
          loadOutput: false,
          loadInput: false,
        });
        if (!status) fail('not_found', '操作不存在');
        const subjectId = status.attributes?.subjectId;
        if (typeof subjectId === 'string')
          await authorize(ctx, 'operations.read', {
            product: 'nous',
            installationId,
            subjectId,
            kind: 'Subject',
            id: subjectId,
          });
        const detail =
          status.status === 'ERROR' ? await DBOS.getWorkflowStatus(id) : status;
        return describeDurableWorkflow(detail ?? status);
      },
      async trace(ctx: CallContext, id: string) {
        const operation = await operations.inspect(ctx, id);
        const steps = describeDurableSteps(await DBOS.listWorkflowSteps(id));
        return { operation, steps };
      },
      async cancel(ctx: CallContext, id: string) {
        await authorize(ctx, 'operations.cancel');
        await operations.inspect(ctx, id);
        await DBOS.cancelWorkflow(id);
        return operations.inspect(ctx, id);
      },
      async resume(ctx: CallContext, id: string) {
        await authorize(ctx, 'operations.resume');
        await operations.inspect(ctx, id);
        await resumeDurableWorkflow(id);
        return operations.inspect(ctx, id);
      },
    };
    const currentTime = async (ctx: CallContext) =>
      (
        await time.capture(ctx, {
          operationId: `${ctx.trace.operationId}/cognition/formed`,
        })
      ).point;
    const sourceDocuments = createSourceDocuments({
      knowledge,
      materials,
      encounters,
    });
    cognition = createCognition(
      owner({
        sourceDocuments,
        knowledge,
        materials,
        encounters,
        retrieval,
        models,
        workerContext,
        currentTime,
        settings,
        capabilities: async (ctx: CallContext) =>
          KnowledgeKindSchema.array().parse(
            (await subjects.read(ctx)).capabilities,
          ),
        operationControl: {
          inspect: operations.inspect,
          pause: operations.cancel,
        },
      }),
    );
    const resources = createResources(owner({ materials }));
    const refresh = async (
      ctx: CallContext,
      views?: Parameters<typeof retrieval.refresh>[1],
    ) =>
      DBOS.isWithinWorkflow()
        ? DBOS.runStep(() => retrieval.refresh(ctx, views), {
            name: 'retrieval.refresh',
            retriesAllowed: false,
          })
        : retrieval.refresh(ctx, views);
    const organization = createOrganization(
      owner({
        sourceDocuments,
        knowledge,
        materials,
        encounters,
        models,
        workerContext,
        refresh,
      }),
    );
    function workflow<I, O>(
      name: string,
      fn: (ctx: CallContext, input: I) => Promise<O>,
      queueName?: string,
    ) {
      const registered = DBOS.registerWorkflow(fn, { name });
      const start = async (ctx: CallContext, input: I) => {
        assertDispatch();
        await authorize(ctx, name);
        const id = `${name}:${ctx.trace.operationId}`;
        const [prior] = await DBOS.listWorkflows({
          workflowIDs: [id],
          loadInput: true,
          loadOutput: false,
        });
        if (prior?.input) {
          const [oldCtx, oldInput] = prior.input as [CallContext, unknown];
          const submission = (context: CallContext, value: unknown) =>
            canonicalize({
              consumer: context.consumer,
              subjectId: context.subjectId ?? null,
              humanId: context.actor.kind === 'human' ? context.actor.id : null,
              input: value,
            });
          if (submission(oldCtx, oldInput) !== submission(ctx, input))
            fail('conflict', '客户端提交 ID 已对应另一输入', {
              operationId: id,
            });
        }
        const handle = await DBOS.startWorkflow(registered, {
          workflowID: id,
          queueName,
          enqueueOptions: queueName
            ? { queuePartitionKey: ctx.subjectId }
            : undefined,
          workflowAttributes: {
            subjectId: ctx.subjectId,
            actorId: ctx.actor.id,
            operation: name,
            traceId: ctx.trace.traceId,
          },
        })(ctx, input);
        return handle;
      };
      return Object.assign(
        async (ctx: CallContext, input: I): Promise<O> =>
          DBOS.isWithinWorkflow()
            ? registered(ctx, input)
            : (await start(ctx, input)).getResult(),
        { start },
      );
    }
    const prepare = cognition.contexts.prepare;
    const materialData=createMaterialData(store,{blobRoot:paths.blob,read:materials.read,resolve:knowledge.sources.resolve,externalHolds:modelData.holdsBlob});
    const promptData=createPromptDataLifecycle(owner({product:'nous' as const}),{overrideRoot:join(paths.config,'prompts')});
    const data=createData(store,{
      productVersion:version,processBootId:telemetry.processBootId,tempRoot:paths.temp,settings,models,
      owners:[createIdentityData(store),materialData,createKnowledgeData(store,{read:knowledge.read}),createCognitionData(store,{resolve:knowledge.sources.resolve}),createResourceData(store),promptData,time.data],
      materials:materialData,artifactBlobs,
      async deleteAuthorities(ctx,subjectId,db){await secrets.deleteSubject(ctx,{subjectId},db);await authentication.deleteSubject(ctx,{subjectId},db);},
      cleanupAuthorities:subjectId=>secrets.cleanupSubject({subjectId}),
      cleanupPrompts:promptData.cleanupSubject,
      removeServing:retrieval.removeSubject,
      rebuildServing:ctx=>{if(options.embedding)throw problem('retrieval.explicit_rebuild_required','unavailable','向量索引须显式选择预算后重建');return retrieval.rebuild(ctx);},
    });
    lifetime.defer(()=>data.close());
    const exportData=workflow('data.export',data.export),importData=workflow('data.import',data.import),purgeData=workflow('data.purge',data.purge);
    const createKnowledgeOperation = async (
      ctx: CallContext,
      input: Parameters<typeof knowledge.create>[1],
    ) => {
      const view = await knowledge.create(ctx, input);
      await refresh(ctx, [view]);
      return view;
    };
    const reviseKnowledgeOperation = async (
      ctx: CallContext,
      input: Parameters<typeof knowledge.revise>[1],
    ) => {
      const view = await knowledge.revise(ctx, input);
      await refresh(ctx, [view]);
      return view;
    };
    const runGrowth = workflow(
      'growth.run',
      cognition.growth.run,
      'nous-growth',
    );
    const resolveGrowth = workflow(
      'growth.resolve',
      cognition.growth.resolve,
      'nous-growth',
    );
    const applyGrowth = workflow(
      'growth.apply',
      cognition.growth.apply,
      'nous-growth',
    );
    const previewSeed = workflow(
      'seeds.preview',
      cognition.seeds.preview,
      'nous-growth',
    );
    const applySeed = workflow(
      'seeds.apply',
      cognition.seeds.apply,
      'nous-growth',
    );
    const runMaintenance = workflow(
      'maintenance.run',
      cognition.maintenance.run,
      'nous-growth',
    );
    const readResource = workflow('resources.read', resources.read);
    const organizeEpisodes = organization.episodes.organize;
    const writeJournal = workflow(
      'journals.write',
      organization.journals.write,
      'nous-growth',
    );
    const mergeConcepts = organization.concepts.merge;
    const splitConcepts = organization.concepts.split;
    const evaluateMotivation = workflow(
      'motivation.evaluate',
      organization.motivation.evaluate,
      'nous-growth',
    );
    async function workerContext(
      ctx: CallContext,
      role: string,
    ): Promise<CallContext> {
      if (ctx.consumer.kind !== 'subject') return ctx;
      if (!ctx.subjectId) fail('invalid_input', '后台调用需要主体范围');
      const parent = await authentication.currentAuthority(ctx);
      const requested = new Date(Date.now() + 5 * 60_000)
        .toISOString()
        .replace('Z', '000Z');
      const expiresAt = {
        domainId: 'real',
        at:
          parent.expiresAt && parent.expiresAt.at < requested
            ? parent.expiresAt.at
            : requested,
      };
      const consumer = {
        kind: 'worker' as const,
        host: installationId,
        id: `${ctx.subjectId}:${ctx.trace.operationId}:${role}`,
      };
      const delegated = await authentication.delegations.create(ctx, {
        name: `cognition:${role}`,
        principalKind: 'worker',
        scope: {
          plane: 'subject',
          operations: subjectOperations.filter(
            (op) => op !== 'delegations.create',
          ),
          subjectIds: [ctx.subjectId],
          consumer,
          purposes: [ctx.purpose],
          audiences: ctx.audience,
          targets: [],
          inputRefs: [],
          expiresAt,
          mayDelegate: false,
        },
      });
      return makeContext({
        ...ctx,
        actor: {
          kind: 'worker',
          id: consumer.id,
          authorityRef: delegated.authorityRef,
        },
        consumer,
        operationId: ctx.trace.operationId,
        traceparent: ctx.trace.traceparent,
      });
    }
    let state:
      | 'created'
      | 'starting'
      | 'running'
      | 'draining'
      | 'stopped'
      | 'failed' = 'created';
    let starting: Promise<void> | undefined;
    let closing: Promise<void> | undefined;
    function assertDispatch() {
      if (state !== 'running' && state !== 'starting')
        throw problem(
          'host.not_accepting_work',
          'unavailable',
          'Host当前不接纳新工作',
        );
    }
    const host = {
      start() {
        if (state === 'stopped')
          throw problem(
            'host.closed',
            'conflict',
            '已关闭的Host不能重新启动，请创建新实例',
          );
        starting ??= (async () => {
          state = 'starting';
          try {
            await DBOS.launch();
            await DBOS.registerQueue('nous-growth', {
              partitionConcurrency: 1,
              workerConcurrency: 2,
            });
            state = 'running';
          } catch (error) {
            state = 'failed';
            throw error;
          }
        })();
        return starting;
      },
      async drain() {
        if (state === 'running') state = 'draining';
        return { state, acceptingWork: false };
      },
      async status() {
        await database.db
          .selectFrom('subjects')
          .select('id')
          .limit(1)
          .execute();
        return {
          product: 'nous' as const,
          installationId,
          version,
          state,
          components: {
            process: { state: 'available' as const },
            database: { state: 'available' as const },
            auth: { state: 'available' as const },
          },
          subjectsAutoStart: false,
        };
      },
      async doctor() {
        await database.db
          .selectFrom('subjects')
          .select('id')
          .limit(1)
          .execute();
        return {
          product: 'nous-wave',
          installationId,
          database: 'available',
          runtime: 'available',
          time: now(),
        };
      },
      stop(mode: 'graceful' | 'force' = 'graceful') {
        closing ??= (async () => {
          await host.drain();
          try {
            await settingsListener.close();
            if (starting)
              await DBOS.shutdown({
                workflowCompletionTimeoutMS: mode === 'force' ? 0 : 30000,
              });
          } finally {
            state = 'stopped';
            await lifetime.disposeAsync();
          }
        })();
        return closing;
      },
    };
    const services = {
      installationId,
      subjects,
      host,
      time,
      materials,
      encounters,
      knowledge: {
        ...knowledge,
        create: createKnowledgeOperation,
        revise: reviseKnowledgeOperation,
        query: retrieval.query,
        expand: retrieval.expand,
        explainQuery: retrieval.explain,
      },
      contexts: { ...cognition.contexts, prepare },
      growth: {
        ...cognition.growth,
        run: runGrowth,
        apply: applyGrowth,
        resolve: resolveGrowth,
      },
      seeds: { ...cognition.seeds, preview: previewSeed, apply: applySeed },
      maintenance: { ...cognition.maintenance, run: runMaintenance },
      feedback: cognition.feedback,
      episodes: { ...organization.episodes, organize: organizeEpisodes },
      journals: { write: writeJournal },
      concepts: { merge: mergeConcepts, split: splitConcepts },
      motivation: { ...organization.motivation, evaluate: evaluateMotivation },
      persona: organization.persona,
      resources: { ...resources, read: readResource },
      settings,
      secrets,
      prompts,
      data:{...data,export:exportData,import:importData,purge:purgeData},
      network,
      testModel,
      nativeModels: nativeHost.facade,
      operations,
      authorize,
      managementScope: () => managementScope(installationId),
      credentials: {
        delegations: authentication.delegations,
        async createManagementKey(ctx: CallContext, name: string) {
          await authorize(ctx, 'installation.manage');
          if (ctx.actor.kind !== 'human')
            fail('forbidden', '管理凭据需要管理员身份');
          return authentication.createManagementKey(ctx.actor.id, {
            name,
            scope: managementScope(installationId),
          });
        },
      },
    };
    return Object.assign(services, { models, telemetry, authentication });
  } catch (error) {
    try {
      await lifetime.disposeAsync();
    } catch (cleanup) {
      throw problem(
        'host.initialization_failed',
        'failed',
        '初始化失败且资源尚未完全关闭',
        {
          details: JSON.parse(
            JSON.stringify({
              initialization: safeProblem(error),
              cleanup: safeProblem(cleanup),
            }),
          ),
        },
      );
    }
    throw error;
  }
}

export async function withNous<T>(
  view: BootstrapView,
  use: (app: Nous) => Promise<T>,
  options: Pick<NousOptions, 'mode'> = {},
): Promise<T> {
  if (view.bootstrap.product !== 'nous')
    throw problem(
      'bootstrap.product_mismatch',
      'conflict',
      '该安装属于另一个产品',
    );
  const secrets = createBootstrapSecrets(view.paths.secret);
  return withBootstrapDatabase(view, (databaseUrl) =>
    secrets.withSecret(
      'auth-signing',
      { purpose: 'auth.signing' },
      async (signingBytes) => {
        const app = await createNous({
          ...options,
          bootstrap: view,
          databaseUrl,
          authSecret: new TextDecoder().decode(signingBytes),
        });
        try {
          return await use(app);
        } finally {
          await app.host.stop();
        }
      },
    ),
  );
}
export { makeContext } from './platform/context.js';

export { DomainError } from './platform/errors.js';
