import type { TimeRecordings } from '@nous-wave/contracts/time';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import type { CallContext } from '@nous-wave/contracts/common';
import { RolePolicySchema, type ModelKind } from '@nous-wave/contracts/models';
import type { Models, RoleDescriptor } from '@nous-wave/platform/models';
import type { Storage } from '../packages/nous/src/platform/context.js';
import { createModelData } from '../packages/nous/src/models/data.js';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { openDatabase } from '../packages/nous/src/platform/database.js';
import { createIdentity } from '../packages/nous/src/identity/index.js';
import { createMaterials } from '../packages/nous/src/materials/index.js';
import { createKnowledge } from '../packages/nous/src/knowledge/index.js';
import { makeContext } from '../packages/nous/src/platform/context.js';
import {
  createTime,
  timeSettingsModule,
} from '../packages/nous/src/time/index.js';
import {
  createModels,
  modelSettingsModules,
  officialProviderAdapters,
} from '../packages/platform/src/models/index.js';
import { createSettings } from '../packages/platform/src/settings/index.js';
import {
  createNetwork,
  networkSettingsModule,
} from '../packages/platform/src/network/index.js';
import { createAuthentication } from '../packages/platform/src/credentials/auth.js';
import { createSecretRegistry } from '../packages/platform/src/credentials/secrets.js';
import {
  createPrompts,
  promptsDefinition,
} from '../packages/platform/src/prompts/index.js';
import { readReal } from '../packages/platform/src/time/index.js';
const require = createRequire(
  new URL('../packages/platform/package.json', import.meta.url),
);
const { DBOS } = require('@dbos-inc/dbos-sdk'),
  { Output, tool, isStepCount } = require('ai'),
  { z } = require('zod');
test('真实五族SDK、工具热配置、DBOS恢复、当前撤权与调用数据闭包', {
  skip: !process.env.NOUS_MODELS_TEST_DATABASE_URL,
}, async () => {
  const databaseURL = process.env.NOUS_MODELS_TEST_DATABASE_URL!;
  const db = await openDatabase(databaseURL, 8);
  const tempRoot = fileURLToPath(new URL('../../temp/', import.meta.url));
  await mkdir(tempRoot, { recursive: true });
  const directory = await mkdtemp(join(tempRoot, 'nous-models-'));
  const installationId = 'models-probe',
    subjectId = '019a05b0-0000-7000-8000-000000000001';
  const operations = [
    'data.manage',
    'data.export',
    'data.import',
    'data.purge',
    'policy.update',
    'subjects.create',
    'subjects.read',
    'materials.capture',
    'materials.read',
    'materials.publish',
    'encounters.accept',
    'encounters.read',
    'knowledge.read',
    'settings.read',
    'settings.update',
    'installation.manage',
    'providers.read',
    'providers.manage',
    'models.read',
    'models.manage',
    'models.invoke',
    'models.invocations.read',
    'models.reconcile',
    'roles.read',
    'costs.read',
    'secrets.manage',
    'delegations.create',
    'delegations.revoke',
    'clock.read',
    'clock.capture',
    'clock.references.read',
  ];
  const authentication = await createAuthentication({
    pool: db.pool,
    db: db.db,
    product: 'nous',
    installationId,
    secret: 'models-infrastructure-probe-0123456789abcdef0123456789',
    baseURL: 'http://127.0.0.1:4333',
    trustedOrigins: [],
    operationPolicy: (operation) =>
      operations.includes(operation)
        ? {
            allowedPlanes:
              ['models.invoke', 'models.invocations.read'].includes(
                operation,
              ) ||
              operation.startsWith('materials.') ||
              operation.startsWith('encounters.') ||
              operation.startsWith('clock.')
                ? ['system', 'subject']
                : ['system'],
            targetRequired: operation === 'models.invoke',
          }
        : undefined,
    issuancePolicy: ({ requested }) =>
      requested.plane === 'subject' &&
      requested.subjectIds.length === 1 &&
      requested.subjectIds[0] === subjectId,
  });
  const user = await authentication.bootstrapAdministrator({
    email: 'models@example.test',
    password: 'models-infrastructure-probe-password-0123456789',
    name: 'Model infrastructure',
  });
  const expiresAt = {
    domainId: 'real',
    at: new Date(Date.now() + 3600000).toISOString(),
  };
  const key = await authentication.createManagementKey(user.userId, {
    name: 'Models probe',
    scope: {
      plane: 'system',
      operations,
      subjectIds: [],
      consumer: { kind: 'maintenance', host: 'nous', id: installationId },
      purposes: ['*'],
      audiences: [subjectId],
      targets: [],
      inputRefs: [],
      expiresAt,
      mayDelegate: true,
    },
  });
  const adminPrincipal = await authentication.authenticate(
    new Headers({ 'x-api-key': key.key }),
  );
  const admin = (): CallContext =>
    makeContext({
      ...adminPrincipal,
      subjectId,
      purpose: 'model',
      audience: [subjectId],
    });
  const store: Storage = {
    db: db.db,
    installationId,
    authorize: authentication.authorize,
    transaction: (use) => db.db.transaction().execute(use),
  };
  const identity = createIdentity(store);
  await identity.create(admin(), {
    id: subjectId,
    displayName: 'SDK真实五族',
    capabilities: ['Memory'],
  });
  let knowledge: ReturnType<typeof createKnowledge>;
  const { materials, encounters, sourceRepository, recordings } =
    createMaterials({
      ...store,
      blobPath: directory + '/blob',
      readableSources: (ctx, refs) => knowledge.sources.readable(ctx, refs),
      resolveSources: (ctx, refs) => knowledge.sources.resolve(ctx, refs),
      inheritSources: (ctx, refs) => knowledge.sources.inherit(ctx, refs),
      externalPolicies: (ctx, refs) =>
        knowledge.sources.externalPolicies(ctx, refs),
    });
  knowledge = createKnowledge({ ...store, sourceRepository });
  const defaults = (kind: ModelKind) =>
    RolePolicySchema.parse({
      kind,
      enabled: false,
      resources: [],
      selection: 'ordered',
      timeoutMs: 30000,
      inference:
        kind === 'language'
          ? { maxOutputTokens: 256, providerOptions: {} }
          : kind === 'embedding'
            ? { maxParallelCalls: 2, providerOptions: {} }
            : { providerOptions: {} },
    });
  const roles: RoleDescriptor[] = (
    [
      'language',
      'embedding',
      'reranking',
      'transcription',
      'speech',
    ] as ModelKind[]
  ).map((kind) => ({
    id: kind,
    label: kind,
    description: kind,
    kind,
    requiredCapabilities: [],
    defaultPolicy: defaults(kind),
  }));
  const modules = modelSettingsModules(roles, officialProviderAdapters).modules;
  const settings = createSettings({
    db: db.db,
    installationId,
    modules: [
      ...modules,
      networkSettingsModule,
      timeSettingsModule,
      promptsDefinition,
    ],
    authorize: (ctx, request, client) =>
      authentication.authorize(
        ctx,
        `settings.${request.action}`,
        undefined,
        client,
      ),
  });
  await settings.registerDefinitions();
  const prompts = createPrompts({
    db: db.db,
    installationId,
    programRoot: directory + '/program',
    overrideRoot: directory + '/override',
    catalog: [],
    settings,
    authorize: async () => {},
    processBootId: 'probe',
  });
  await mkdir(directory + '/program', { recursive: true });
  await prompts.initialize();
  let timeRecordings!: TimeRecordings;
  const clock = createTime(store, {
    settings,
    processBootId: 'probe',
    recordings: (capability) => {
      timeRecordings = capability;
    },
  });
  let models: Models;
  const secrets = createSecretRegistry({
    db: db.db,
    product: 'nous',
    installationId,
    secretRoot: directory + '/secret',
    authorize: authentication.authorize,
    purposes: {
      'model.provider': {
        operation: 'models.invoke',
        resourceKinds: ['Provider'],
        resolveUse: (ctx, input) => models.credentialTarget(ctx, input),
      },
    },
  });
  const network = createNetwork({
    product: 'nous',
    installationId,
    processBootId: 'probe',
    settings,
    authorize: authentication.authorize,
  });
  type ProviderBody = {
    model?: string;
    input?: string[];
    messages?: Array<{ role: string }>;
    tools?: unknown[];
    stream?: boolean;
    max_completion_tokens?: number;
    max_tokens?: number;
  };
  let loseTerminalConfirmation = false;
  let onProviderRequest: (() => Promise<void>) | undefined;
  let calls = 0,
    toolRequests = 0;
  let lastBody: ProviderBody = {},
    invalid = false,
    fail = false,
    failAdopt = false;
  const provider = createServer(async (request, response) => {
    assert.equal(request.headers.authorization, 'Bearer one-provider-key');
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const bytes = Buffer.concat(chunks);
    let body: ProviderBody;
    try {
      body = JSON.parse(bytes.toString());
    } catch {
      body = {};
    }
    lastBody = body;
    calls++;
    const callback = onProviderRequest;
    onProviderRequest = undefined;
    if (callback) await callback();
    if (fail) {
      response.writeHead(429, {
        'content-type': 'application/json',
        'x-request-id': 'safe-id',
      });
      response.end(
        JSON.stringify({
          error: { message: 'private-provider-error-echo', type: 'rate_limit' },
        }),
      );
      return;
    }
    if (request.url === '/v1/embeddings') {
      assert.ok(body.input);
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          object: 'list',
          model: body.model,
          data: body.input.map((value: string, index: number) => ({
            object: 'embedding',
            index,
            embedding: [value.length, 1],
          })),
          usage: {
            prompt_tokens: body.input.length,
            total_tokens: body.input.length,
          },
        }),
      );
      return;
    }
    if (request.url === '/v2/rerank') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          id: 'rerank-1',
          results: [
            { index: 1, relevance_score: 0.9 },
            { index: 0, relevance_score: 0.1 },
          ],
          meta: { billed_units: { search_units: 1 } },
        }),
      );
      return;
    }
    if (request.url === '/v1/audio/transcriptions') {
      assert.match(request.headers['content-type'] ?? '', /multipart/);
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          text: '实际音频转录',
          language: 'chinese',
          duration: 1.25,
          segments: [
            {
              id: 0,
              seek: 0,
              start: 0,
              end: 1.25,
              text: '实际音频转录',
              tokens: [1],
              temperature: 0,
              avg_logprob: -0.1,
              compression_ratio: 1,
              no_speech_prob: 0,
            },
          ],
        }),
      );
      return;
    }
    if (request.url === '/v1/audio/speech') {
      response.writeHead(200, { 'content-type': 'audio/mpeg' });
      response.end(Buffer.from([0x49, 0x44, 0x33, 1, 2, 3]));
      return;
    }
    assert.equal(request.url, '/v1/chat/completions');
    assert.ok(
      JSON.stringify(body).includes('青柠咖啡馆'),
      'actual SDK request retains original text',
    );
    assert.ok(body.messages);
    const hasTool = body.messages.some(
      (message: { role: string }) => message.role === 'tool',
    );
    const useTool = !!body.tools?.length && !hasTool;
    const content = invalid
      ? 'actual-not-json'
      : JSON.stringify({ answer: hasTool ? '已查到周六' : '青柠咖啡馆' });
    if (useTool) toolRequests++;
    if (body.stream) {
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      for (const chunk of [
        {
          id: 'stream-1',
          object: 'chat.completion.chunk',
          created: 1,
          model: body.model,
          choices: [
            {
              index: 0,
              delta: { role: 'assistant', content: '青柠咖啡馆' },
              finish_reason: null,
            },
          ],
        },
        {
          id: 'stream-1',
          object: 'chat.completion.chunk',
          created: 1,
          model: body.model,
          choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
          usage: { prompt_tokens: 30, completion_tokens: 10, total_tokens: 40 },
        },
      ])
        response.write(`data: ${JSON.stringify(chunk)}\n\n`);
      response.end('data: [DONE]\n\n');
      return;
    }
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(
      JSON.stringify({
        id: 'completion-' + calls,
        object: 'chat.completion',
        created: 1,
        model: body.model,
        choices: [
          {
            index: 0,
            message: useTool
              ? {
                  role: 'assistant',
                  content: null,
                  tool_calls: [
                    {
                      id: 'tool-1',
                      type: 'function',
                      function: { name: 'lookup', arguments: '{}' },
                    },
                  ],
                }
              : { role: 'assistant', content },
            finish_reason: useTool ? 'tool_calls' : 'stop',
          },
        ],
        usage: { prompt_tokens: 30, completion_tokens: 10, total_tokens: 40 },
      }),
    );
  });
  await new Promise<void>((resolve) =>
    provider.listen(0, '127.0.0.1', resolve),
  );
  const address = provider.address();
  assert.ok(address && typeof address !== 'string');
  const baseURL = `http://127.0.0.1:${address.port}`;
  const data = createModelData({
    installationId,
    db: db.db,
    blobRoot: directory + '/blob',
    authorize: authentication.authorize,
    materials,
    recordings,
    validateSources: async (ctx, refs) => {
      await knowledge.sources.resolve(ctx, [...refs]);
    },
  });
  models = createModels({
    product: 'nous',
    installationId,
    processBootId: 'probe',
    db: db.db,
    settings,
    credentials: secrets,
    network,
    prompts,
    roleCatalog: roles,
    providerAdapters: officialProviderAdapters,
    authorize: authentication.authorize,
    clock: {
      ...clock,
      captureTerminal: async (ctx, input) => {
        const receipt = await timeRecordings.captureTerminal(ctx, input);
        if (loseTerminalConfirmation) {
          loseTerminalConfirmation = false;
          throw Error('terminal receipt confirmation lost');
        }
        return receipt;
      },
    },
    budgetScopes: {
      resolve: async () => ({
        constraints: [
          {
            key: 'installation:model-limit',
            budget: { maxCalls: 100, concurrentCalls: 4 },
          },
        ],
      }),
    },
    data: {
      ...data,
      adopt: async (ctx, input) => {
        await data.adopt(ctx, input);
        if (failAdopt) {
          failAdopt = false;
          throw Error('fixture-adoption-failure');
        }
      },
    },
  });
  try {
    const installation = { kind: 'installation' as const, installationId };
    const credential = await secrets.create(admin(), {
      purpose: 'model.provider',
      resourceRef: {
        product: 'nous',
        installationId,
        kind: 'Provider',
        id: 'openai',
      },
      allowedConsumers: [{ kind: 'worker', host: 'nous', id: '*' }],
      backend: {
        kind: 'protected_file',
        material: Buffer.from('one-provider-key'),
      },
    });
    const cohereCredential = await secrets.create(admin(), {
      purpose: 'model.provider',
      resourceRef: {
        product: 'nous',
        installationId,
        kind: 'Provider',
        id: 'cohere',
      },
      allowedConsumers: [{ kind: 'worker', host: 'nous', id: '*' }],
      backend: {
        kind: 'protected_file',
        material: Buffer.from('one-provider-key'),
      },
    });
    await models.providers.create(admin(), {
      expectedRevision: 0,
      definition: {
        id: 'openai',
        adapter: 'openai',
        baseURL: baseURL + '/v1',
        credentialRef: credential.ref,
        networkProfileId: 'default',
        options: {
          mode: 'chat',
          addressScope: 'configured_private',
          allowedPrivateRanges: ['127.0.0.0/8'],
        },
        enabled: true,
      },
    });
    await models.providers.create(admin(), {
      expectedRevision: 1,
      definition: {
        id: 'cohere',
        adapter: 'cohere',
        baseURL: baseURL + '/v2',
        credentialRef: cohereCredential.ref,
        networkProfileId: 'default',
        options: {
          addressScope: 'configured_private',
          allowedPrivateRanges: ['127.0.0.0/8'],
        },
        enabled: true,
      },
    });
    const resource = (id: string, kind: ModelKind, providerId = 'openai') => ({
      managedBy: 'sdk',
      definition: {
        id,
        label: id,
        providerId,
        modelId:
          kind === 'language'
            ? 'gpt-4o'
            : kind === 'embedding'
              ? 'text-embedding-3-small'
              : kind === 'reranking'
                ? 'rerank-v3.5'
                : kind === 'transcription'
                  ? 'whisper-1'
                  : 'tts-1',
        kind,
        enabled: true,
        capabilities: [],
        contextLimit: 65536,
        outputLimit: 4096,
        providerOptions: {},
        ...(kind === 'language' ? { pricingRef: 'text' } : {}),
      },
    });
    await settings.update(admin(), {
      namespace: 'platform.models',
      scope: installation,
      expectedRevision: 0,
      patch: [
        {
          op: 'add',
          path: '/resources',
          value: Object.fromEntries([
            ...roles.map((role) => [
              role.id,
              resource(
                role.id,
                role.kind,
                role.kind === 'reranking' ? 'cohere' : 'openai',
              ),
            ]),
            ['language-second', resource('language-second', 'language')],
          ]),
        },
      ],
    });
    await settings.update(admin(), {
      namespace: 'platform.modelPrices',
      scope: installation,
      expectedRevision: 0,
      patch: [
        {
          op: 'add',
          path: '/prices',
          value: {
            text: {
              id: 'text',
              rates: [
                {
                  unit: 'input_tokens',
                  amount: { amount: '0.1', currency: 'USD' },
                  per: '1000000',
                },
                {
                  unit: 'output_tokens',
                  amount: { amount: '0.2', currency: 'USD' },
                  per: '1000000',
                },
              ],
            },
          },
        },
      ],
    });
    await settings.update(admin(), {
      namespace: 'platform.roles',
      scope: installation,
      expectedRevision: 0,
      patch: [
        {
          op: 'add',
          path: '/roles',
          value: Object.fromEntries(
            roles.map((role) => [
              role.id,
              {
                ...defaults(role.kind),
                enabled: true,
                resources: [{ id: role.id, revision: 1 }],
              },
            ]),
          ),
        },
      ],
    });
    const workerOperations = [
      'models.invocations.read',
      'models.invoke',
      'materials.capture',
      'materials.read',
      'encounters.accept',
      'encounters.read',
      'clock.read',
      'clock.capture',
      'clock.references.read',
    ];
    const issued = await authentication.delegations.create(admin(), {
      name: 'Actual SDK worker',
      principalKind: 'worker',
      scope: {
        plane: 'subject',
        operations: workerOperations,
        subjectIds: [subjectId],
        consumer: { kind: 'worker', host: 'nous', id: 'model-worker' },
        purposes: ['model'],
        audiences: [subjectId],
        targets: [
          {
            kind: 'object',
            refs: [
              ...roles.map((role) => ({
                product: 'nous' as const,
                installationId,
                kind: 'ModelResource',
                id: role.id,
              })),
              {
                product: 'nous' as const,
                installationId,
                kind: 'ModelResource',
                id: 'language-second',
              },
            ],
            actions: ['models.invoke'],
          },
        ],
        inputRefs: [],
        expiresAt,
        mayDelegate: false,
      },
    });
    const workerPrincipal = await authentication.authenticate(
      new Headers({ 'x-api-key': issued.key }),
    );
    const worker = (): CallContext =>
      makeContext({
        ...workerPrincipal,
        subjectId,
        purpose: 'model',
        audience: [subjectId],
      });
    const originalMessage = await materials.capture(admin(), {
      kind: 'message',
      mediaType: 'text/plain',
      text: '小林约我周六在青柠咖啡馆见面。',
      source: { roots: [] },
    });
    const message = await materials.publish(admin(), {
      ref: originalMessage.ref,
    });
    const request = (kind: ModelKind) => ({
      roleId: kind,
      inputRefs: [message.ref],
      options: {},
    });
    const first = await models.text.generate(worker(), {
      ...request('language'),
      options: {
        messages: [{ role: 'user', content: '请读取青柠咖啡馆的约定。' }],
        output: Output.object({ schema: z.object({ answer: z.string() }) }),
      },
    });
    assert.equal(
      z.object({ answer: z.string() }).parse(first.output).answer,
      '青柠咖啡馆',
    );
    assert.equal(lastBody.max_completion_tokens ?? lastBody.max_tokens, 256);
    const revocable = await authentication.delegations.create(admin(), {
      name: '进行中实际调用的撤权',
      principalKind: 'worker',
      scope: {
        plane: 'subject',
        operations: workerOperations,
        subjectIds: [subjectId],
        consumer: {
          kind: 'worker',
          host: 'nous',
          id: 'revocable-model-worker',
        },
        purposes: ['model'],
        audiences: [subjectId],
        targets: [],
        inputRefs: [],
        expiresAt,
        mayDelegate: false,
      },
    });
    const revocablePrincipal = await authentication.authenticate(
      new Headers({ 'x-api-key': revocable.key }),
    );
    const revocableContext = makeContext({
      ...revocablePrincipal,
      subjectId,
      purpose: 'model',
      audience: [subjectId],
    });
    onProviderRequest = async () => {
      await authentication.delegations.revoke(admin(), revocable.delegationId, {
        expectedRevision: 1,
      });
    };
    loseTerminalConfirmation = true;
    await assert.rejects(
      models.text.generate(revocableContext, {
        ...request('language'),
        options: {
          messages: [
            {
              role: 'user',
              content: '实际读取青柠咖啡馆约定，调用进行中撤回委派。',
            },
          ],
        },
      }),
    );
    const terminated = (
      await models.invocations.list(admin(), {
        logicalOperationId: revocableContext.trace.operationId,
      })
    ).items[0]!;
    assert.equal(terminated.status, 'completed');
    assert.equal(terminated.formed, undefined);
    const reconciler = makeContext({
      ...adminPrincipal,
      purpose: 'management',
      audience: [subjectId],
    });
    const resumedFact = await models.reconcile(reconciler, {
      invocationId: terminated.identity.invocationId,
      status: 'completed',
    });
    assert.ok(resumedFact.formed);
    const actualEncounter = (await encounters.list(admin())).find(
      (row) => row.body.invocationId === terminated.identity.invocationId,
    )!;
    assert.deepEqual(actualEncounter.body.consumer, revocableContext.consumer);
    assert.equal(actualEncounter.body.purpose, 'model');
    assert.ok(
      terminated.observed && resumedFact.formed && terminated.actualCost,
    );
    assert.ok(terminated.outputRefs.length);
    assert.match(
      (await materials.read(admin(), { ref: terminated.outputRefs.at(-1)! }))
        .text ?? '',
      /青柠咖啡馆/,
    );
    await assert.rejects(
      materials.read(revocableContext, { ref: terminated.outputRefs.at(-1)! }),
    );
    await assert.rejects(
      materials.publish(admin(), { ref: terminated.outputRefs[0]! }),
      /可读成果投影/,
    );
    const publicResult = await materials.publish(admin(), {
      ref: terminated.outputRefs.at(-1)!,
    });
    assert.equal(publicResult.blobRef, undefined);

    const beforeDenied = calls;
    await assert.rejects(
      models.text.generate(
        {
          ...revocableContext,
          trace: {
            ...revocableContext.trace,
            operationId: revocableContext.trace.operationId + '/denied',
          },
        },
        { ...request('language'), options: { prompt: '青柠咖啡馆后续调用。' } },
      ),
    );
    assert.equal(
      calls,
      beforeDenied,
      '撤权后的新调用不派发，已发生结果/时间/费用保全',
    );

    const second = worker();
    await settings.update(admin(), {
      namespace: 'platform.roles',
      scope: installation,
      expectedRevision: 1,
      patch: [
        {
          op: 'replace',
          path: '/roles/language/resources',
          value: [{ id: 'language-second', revision: 1 }],
        },
      ],
    });
    await models.text.generate(second, {
      ...request('language'),
      options: { messages: [{ role: 'user', content: '读取青柠咖啡馆。' }] },
    });
    const toolResult = await models.text.generate(worker(), {
      ...request('language'),
      options: {
        messages: [{ role: 'user', content: '查询青柠咖啡馆的见面时间。' }],
        tools: {
          lookup: tool({
            description: '读取约定时间',
            inputSchema: z.object({}),
            execute: async () => {
              await settings.update(admin(), {
                namespace: 'platform.roles',
                scope: installation,
                expectedRevision: 2,
                patch: [
                  {
                    op: 'replace',
                    path: '/roles/language/inference/maxOutputTokens',
                    value: 192,
                  },
                ],
              });
              return { day: '周六' };
            },
          }),
        },
        stopWhen: isStepCount(3),
      },
    });
    assert.equal(toolRequests, 1);
    assert.equal(toolResult.steps.length, 2);
    assert.equal(
      lastBody.max_completion_tokens ?? lastBody.max_tokens,
      192,
      'next real SDK step applies fresh Role settings',
    );
    const stream = await models.text.stream(worker(), {
      ...request('language'),
      options: {
        messages: [{ role: 'user', content: '流式阅读青柠咖啡馆。' }],
      },
    });
    let text = '';
    for await (const part of stream.textStream) text += part;
    assert.equal(text, '青柠咖啡馆');
    assert.deepEqual(
      (
        await models.embeddings.embed(worker(), {
          ...request('embedding'),
          options: { value: '青柠咖啡馆' },
        })
      ).embedding,
      [5, 1],
    );
    assert.equal(
      (
        await models.reranking.rerank(worker(), {
          ...request('reranking'),
          options: { query: '咖啡馆', documents: ['火车站', '青柠咖啡馆'] },
        })
      ).ranking[0].originalIndex,
      1,
    );
    assert.equal(
      (
        await models.transcription.transcribe(worker(), {
          ...request('transcription'),
          options: { audio: new Uint8Array([1, 2, 3]) },
        })
      ).text,
      '实际音频转录',
    );
    const spoken = await models.speech.generate(worker(), {
      ...request('speech'),
      options: { text: '青柠咖啡馆', voice: 'alloy' },
    });
    assert.deepEqual([...spoken.audio.uint8Array], [0x49, 0x44, 0x33, 1, 2, 3]);
    const broken = worker();
    invalid = true;
    await assert.rejects(
      models.text.generate(broken, {
        ...request('language'),
        options: {
          messages: [{ role: 'user', content: '青柠咖啡馆结构答复。' }],
          output: Output.object({ schema: z.object({ answer: z.string() }) }),
        },
      }),
      (error) =>
        (error as { problem?: { code: string } }).problem?.code ===
          'models.output_invalid' &&
        !JSON.stringify(error).includes('actual-not-json'),
    );
    invalid = false;
    const settled = (await models.invocations.list(admin())).items.find(
      (call) => call.identity.logicalOperationId === broken.trace.operationId,
    );
    assert.equal(settled!.status, 'completed');
    assert.ok(settled!.actualCost);
    assert.ok(settled!.outputRefs.length);
    const unknown = worker();
    fail = true;
    const before = calls;
    await assert.rejects(
      models.text.generate(unknown, {
        ...request('language'),
        options: { messages: [{ role: 'user', content: '青柠咖啡馆失败。' }] },
      }),
      (error) => !JSON.stringify(error).includes('private-provider-error-echo'),
    );
    await assert.rejects(
      models.text.generate(unknown, {
        ...request('language'),
        options: { messages: [{ role: 'user', content: '青柠咖啡馆失败。' }] },
      }),
    );
    assert.equal(calls, before + 1);
    fail = false;
    DBOS.setConfig({
      name: 'shared-models-probe',
      systemDatabaseUrl: databaseURL,
      applicationVersion: 'models-probe-1',
      enableOTLP: false,
    });
    const batchWorkflow = DBOS.registerWorkflow(
      async (ctx: CallContext) =>
        models.embeddings.embedMany(ctx, {
          ...request('embedding'),
          options: {
            values: Array.from(
              { length: 2049 },
              (_, index) => 'batch-' + index,
            ),
            maxParallelCalls: 2,
          },
        }),
      { name: 'probe.embedding-batches' },
    );
    const adoptionWorkflow = DBOS.registerWorkflow(
      async (ctx: CallContext) => {
        const result = await models.text.generate(ctx, {
          ...request('language'),
          options: {
            messages: [{ role: 'user', content: '恢复青柠咖啡馆实际答复。' }],
            output: Output.object({ schema: z.object({ answer: z.string() }) }),
          },
        });
        return result.output;
      },
      { name: 'probe.adoption-recovery' },
    );
    await DBOS.launch();
    const batchContext = worker(),
      beforeBatch = calls;
    const batchHandle = await DBOS.startWorkflow(batchWorkflow, {
      workflowID: 'probe-embedding-batches',
    })(batchContext);
    const embedded = await batchHandle.getResult();
    assert.equal(embedded.embeddings.length, 2049);
    assert.equal(calls, beforeBatch + 2);
    assert.equal(
      (
        await DBOS.startWorkflow(batchWorkflow, {
          workflowID: 'probe-embedding-batches',
        })(batchContext)
      ).workflowID,
      batchHandle.workflowID,
    );
    assert.equal(calls, beforeBatch + 2);
    const adoptionContext = worker(),
      beforeRecovery = calls;
    failAdopt = true;
    const adoption = await DBOS.startWorkflow(adoptionWorkflow, {
      workflowID: 'probe-adoption-recovery',
    })(adoptionContext);
    await assert.rejects(adoption.getResult());
    const known = (await models.invocations.list(admin())).items.find(
      (call) =>
        call.identity.logicalOperationId === adoptionContext.trace.operationId,
    );
    assert.equal(known!.status, 'completed');
    assert.ok(known!.outputRefs.length);
    const resumed = await DBOS.rewindWorkflow('probe-adoption-recovery', {
      startStep: 0,
    });
    assert.equal((await resumed.getResult()).answer, '青柠咖啡馆');
    assert.equal(
      calls,
      beforeRecovery + 1,
      'rewind uses original output, never Provider again',
    );

    const originalOptions = {
      messages: [
        { role: 'user' as const, content: '恢复青柠咖啡馆实际答复。' },
      ],
      output: Output.object({ schema: z.object({ answer: z.string() }) }),
    };
    const providerView = await models.providers.read(admin(), { id: 'openai' });
    await models.providers.update(admin(), {
      id: 'openai',
      expectedRevision: 2,
      definition: { ...providerView.definition, enabled: false },
    });
    await secrets.revoke(admin(), { ref: credential.ref, expectedRevision: 1 });
    assert.equal(
      z.object({ answer: z.string() }).parse(
        (
          await models.text.generate(adoptionContext, {
            ...request('language'),
            options: originalOptions,
          })
        ).output,
      ).answer,
      '青柠咖啡馆',
      'completed ordinary replay reads actual typed output after Provider disable/key revoke',
    );
    assert.equal(calls, beforeRecovery + 1);
    await assert.rejects(
      models.text.generate(adoptionContext, {
        ...request('language'),
        options: {
          ...originalOptions,
          messages: [{ role: 'user', content: '不同的青柠咖啡馆输入' }],
        },
      }),
      (error) =>
        (error as { problem?: { code: string } }).problem?.code ===
        'models.input_changed',
    );
    assert.equal(calls, beforeRecovery + 1);
    await materials.updatePolicy(admin(), {
      ref: message.ref,
      policy: {
        readers: ['unrelated-reader'],
        purposes: ['model'],
        audiences: [subjectId],
      },
    });
    const denied = await DBOS.rewindWorkflow('probe-adoption-recovery', {
      startStep: 0,
    });
    await assert.rejects(
      denied.getResult(),
      (error) =>
        (error as { problem?: { category: string } }).problem?.category ===
        'forbidden',
    );
    assert.equal(
      calls,
      beforeRecovery + 1,
      'current source denial applies even outside SDK durable replay checkpoint',
    );
    const records = (await models.invocations.list(admin())).items;
    assert.ok(records.some((call) => call.status === 'unknown'));
    assert.ok(
      records
        .filter((call) => call.status === 'completed')
        .every((call) => call.observed && call.formed),
    );
    assert.equal(calls, 15);
    assert.equal(records.length, 15);
    await assert.rejects(models.data.deleteSubject(admin(),{subjectId}),error=>(error as {problem?:{code:string}}).problem?.code==='models.unresolved_calls');
    const unresolved=records.find(call=>call.status==='unknown')!;
    await models.reconcile(admin(),{invocationId:unresolved.identity.invocationId,status:'not_sent'});
    const bundle = await models.data.export(admin(), { subjectId });
    assert.equal(bundle.calls.length, 15);
    const beforeRestore = calls;
    await assert.rejects(
      db.db.transaction().execute(async (tx) => {
        await models.data.deleteSubject(admin(), { subjectId }, tx);
        await models.data.import(admin(), bundle, tx);
        throw Error('rollback real model restore');
      }),
      /rollback real model restore/,
    );
    assert.equal(
      (await models.data.export(admin(), { subjectId })).calls.length,
      15,
    );
    await models.data.deleteSubject(admin(), { subjectId });
    assert.equal(
      (await models.data.export(admin(), { subjectId })).calls.length,
      0,
    );
    assert.equal((await models.data.import(admin(), bundle)).restored, 15);
    assert.equal((await models.data.import(admin(), bundle)).retained, 15);
    const restored = await models.data.export(admin(), { subjectId });
    assert.deepEqual(
      restored.calls.map((call) => ({
        id: call.id,
        usage: call.usage,
        cost: call.actualCost,
        output: call.outputRefs,
      })),
      bundle.calls.map((call) => ({
        id: call.id,
        usage: call.usage,
        cost: call.actualCost,
        output: call.outputRefs,
      })),
    );
    assert.equal(
      calls,
      beforeRestore,
      'restoring call facts never dispatches Provider',
    );
  } finally {
    await DBOS.shutdown();
    await models.close();
    clock.close();
    await prompts.close();
    await network.close();
    await db.close();
    await new Promise<void>((resolve, reject) =>
      provider.close((error) => (error ? reject(error) : resolve())),
    );
    await rm(directory, { recursive: true, force: true });
  }
});
