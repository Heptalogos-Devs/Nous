import { createHash } from 'node:crypto';
import type { Readable } from 'node:stream';
import swagger from '@fastify/swagger';
import swaggerUI from '@fastify/swagger-ui';
import {
  ContextPrepareSchema,
  ContextViewSchema,
  ExpandSchema,
  ExpansionResultSchema,
  FeedbackSchema,
  GrowthResolveSchema,
  QueryResultSchema,
  QuerySchema,
  ResourceBodySchema,
  SeedApplySchema,
  WorkContextBodySchema,
  WorkContextSchema,
} from '@nous-wave/contracts/cognition';
import type { CallContext } from '@nous-wave/contracts/common';
import {
  CallContextSchema,
  ConsumerSchema,
  OperationRefSchema,
  PolicySchema,
  ProblemSchema,
  RefSchema,
  readProblem,
  SelectorQuerySchema,
  TimePointInputSchema,
} from '@nous-wave/contracts/common';
import {
  AuthenticatedIdentitySchema,
  DelegationChildCreateSchema,
  DelegationChildViewSchema,
  DelegationScopeSchema,
  SecretStatusSchema,
} from '@nous-wave/contracts/credentials';
import { SubjectCreateSchema } from '@nous-wave/contracts/identity';
import {
  ConceptMergeSchema,
  ConceptSplitSchema,
  EpisodeOrganizeSchema,
  JournalWriteSchema,
  KnowledgeCreateSchema,
  KnowledgeHistorySchema,
  KnowledgeKindSchema,
  KnowledgePageSchema,
  KnowledgeReadSchema,
  KnowledgeReviseSchema,
  KnowledgeViewSchema,
  MotivationEvaluateSchema,
} from '@nous-wave/contracts/knowledge';
import {
  BlobResultSchema,
  EncounterAcceptSchema,
  EncounterViewSchema,
  MaterialCaptureSchema,
  MaterialPublishSchema,
  MaterialViewSchema,
  SourceResolutionSchema,
  SourcesResolveSchema,
} from '@nous-wave/contracts/materials';
import {
  InvocationSnapshotSchema,
  ModelCallViewSchema,
  ModelInvocationFilterSchema,
  ModelInvocationPageSchema,
  ModelReconcileSchema,
  ModelRegistrationSchema,
  ModelTestInputSchema,
  NativeInvocationCompletionSchema,
  NativeInvocationPrepareSchema,
  ProviderDefinitionSchema,
  ResourceRefSchema,
} from '@nous-wave/contracts/models';
import {
  PromptActivateSchema,
  PromptChangeResultSchema,
  PromptDescribeSchema,
  PromptDescriptionSchema,
  PromptHistoryPageSchema,
  PromptHistorySchema,
  PromptListPageSchema,
  PromptListSchema,
  PromptPreviewResultSchema,
  PromptPreviewSchema,
  PromptReadResultSchema,
  PromptReadSchema,
  PromptRenderRequestSchema,
  PromptRenderResultSchema,
  PromptResetSchema,
  PromptSaveSchema,
  PromptSnapshotRequestSchema,
  PromptSnapshotSchema,
} from '@nous-wave/contracts/prompts';
import {
  HostStatusSchema,
  OperationViewSchema,
  OperationControlSchema,
  OperationTraceSchema,
} from '@nous-wave/contracts/runtime';
import {
  ConfigScopeSchema,
  ConfigSnapshotSchema,
  ConsumerAppliedViewSchema,
  EffectiveSettingsViewSchema,
  ModuleDescriptionSchema,
  SettingsChangeResultSchema,
  SettingsChangeSchema,
  SettingsDescribeSchema,
  SettingsHistoryPageSchema,
  SettingsHistorySchema,
  SettingsReadSchema,
  SettingsResetSchema,
  SettingsRestoreSchema,
} from '@nous-wave/contracts/settings';
import {
  CapturedTimeSchema,
  TimeCaptureReceiptSchema,
  TimeCaptureRequestSchema,
  TimeSelectionSchema,
} from '@nous-wave/contracts/time';
import type { Nous } from '@nous-wave/nous';
import { DomainError, makeContext } from '@nous-wave/nous';
import { problem } from '@nous-wave/platform/errors';
import Fastify, {
  type FastifyBaseLogger,
  type FastifyInstance,
  LogController,
  type RawServerDefault,
} from 'fastify';
import {
  createJsonSchemaTransform,
  createJsonSchemaTransformObject,
  serializerCompiler,
  validatorCompiler,
} from 'fastify-type-provider-zod';
import qs from 'qs';
import { v7 as uuid } from 'uuid';
import { z } from 'zod';

declare module 'fastify' {
  interface FastifyRequest {
    callContext?: CallContext;
  }
}

export async function createHttp(
  application: Nous | undefined = undefined,
  options: {
    baseURL?: string;
    authentication?: boolean;
    stop?: (mode: 'graceful' | 'force') => Promise<void>;
  } = {},
): Promise<FastifyInstance> {
  const app = () => {
    if (!application)
      throw new Error('Schema-only server cannot execute business operations');
    return application;
  };
  const server = Fastify<
    RawServerDefault,
    import('node:http').IncomingMessage,
    import('node:http').ServerResponse,
    FastifyBaseLogger
  >({
    genReqId: () => uuid(),
    logController: new LogController({ disableRequestLogging: true }),
    routerOptions: {
      querystringParser: (value) =>
        qs.parse(value, {
          allowPrototypes: false,
          plainObjects: true,
          depth: 8,
          strictDepth: true,
          parameterLimit: 128,
          arrayLimit: 100,
          throwOnLimitExceeded: true,
        }),
    },
    ...(application
      ? {
          loggerInstance: application.telemetry.component('http'),
        }
      : { logger: false }),
  });
  server.addContentTypeParser('*', (_request, payload, done) =>
    done(null, payload),
  );
  server.setValidatorCompiler(validatorCompiler);
  server.setSerializerCompiler(serializerCompiler);
  await server.register(swagger, {
    openapi: {
      openapi: '3.0.3',
      info: { title: 'Nous Wave', version: '0.2.0-infrastructure.1' },
      components: {
        securitySchemes: { bearer: { type: 'http', scheme: 'bearer' } },
      },
    },
    transform: createJsonSchemaTransform({
      zodToJsonConfig: { target: 'openapi-3.0' },
    }),
    transformObject: createJsonSchemaTransformObject({
      zodToJsonConfig: { target: 'openapi-3.0' },
    }),
  });
  await server.register(swaggerUI, { routePrefix: '/docs' });
  const authentication =
    !application || options.authentication === false
      ? undefined
      : app().authentication;
  if (authentication)
    server.route({
      method: ['GET', 'POST'],
      url: '/v1/auth/*',
      schema: { hide: true },
      handler: async (request, reply) => {
        const path = request.url.split('?')[0]!;
        if (path.includes('/sign-up') || path.includes('/api-key/'))
          return reply.code(403).type('application/problem+json').send({
            code: 'credentials.native_endpoint_disabled',
            category: 'forbidden',
            status: 403,
            messageKey: 'credentials.native_endpoint_disabled',
            message: '此操作使用正式管理入口',
          });
        const url = new URL(
          request.url,
          options.baseURL ?? 'http://127.0.0.1:4333',
        );
        const headers = new Headers();
        for (const [name, value] of Object.entries(request.headers))
          if (value !== undefined)
            headers.set(name, Array.isArray(value) ? value.join(',') : value);
        const response = await authentication.handler(
          new Request(url, {
            method: request.method,
            headers,
            ...(request.method !== 'GET'
              ? { body: JSON.stringify(request.body ?? {}) }
              : {}),
          }),
        );
        reply.code(response.status);
        for (const [name, value] of response.headers)
          if (name !== 'set-cookie') reply.header(name, value);
        const cookies = response.headers.getSetCookie();
        if (cookies.length) reply.header('set-cookie', cookies);
        return reply.send(await response.text());
      },
    });
  server.decorateRequest('callContext');
  server.setErrorHandler((error, request, reply) => {
    let failure: z.infer<typeof ProblemSchema> | undefined;
    const known = readProblem(error);
    if (known) failure = known;
    else if (error instanceof Error && 'problem' in error) {
      const parsed = ProblemSchema.safeParse(error.problem);
      if (parsed.success) failure = parsed.data;
    }
    if (!failure) {
      const validation =
        error instanceof z.ZodError ||
        (error instanceof Error && 'validation' in error);
      failure = {
        code: validation ? 'http.invalid_input' : 'system.failed',
        category: validation ? ('validation' as const) : ('failed' as const),
        status: validation ? 400 : 500,
        messageKey: validation ? 'http.invalid_input' : 'system.failed',
        message: validation ? '请求字段无效' : '操作执行失败',
      };
    }
    const ctx = request.callContext;
    failure = {
      operationId: ctx?.trace.operationId ?? request.id,
      ...(ctx ? { traceId: ctx.trace.traceId } : {}),
      ...failure,
    };
    request.log.error({ event: 'http.failed', problem: failure });
    return reply
      .type('application/problem+json')
      .code(failure.status)
      .send(failure);
  });
  const S = '/v1/subjects/:subjectId';
  const params = z.object({ subjectId: z.uuidv7() });
  type RouteIds = { subjectId?: string; id?: string; action?: string };
  async function context(request: {
    headers: Record<string, string | string[] | undefined>;
    params: unknown;
    url: string;
    method?: string;
    query?: unknown;
    body?: unknown;
  }): Promise<CallContext> {
    if (!authentication)
      throw new DomainError(
        'forbidden',
        '未启用认证的 schema 构建实例不接受业务请求',
      );
    const headers = new Headers();
    for (const [name, value] of Object.entries(request.headers))
      if (value !== undefined)
        headers.set(name, Array.isArray(value) ? value.join(',') : value);
    const { actor, consumer } = await authentication.authenticate(headers);
    const selected = (
      request.method === 'GET' ? request.query : request.body
    ) as { subjectId?: unknown; scope?: { subjectId?: unknown } } | undefined;
    const subjectId =
      (request.params as { subjectId?: string })?.subjectId ??
      (selected?.scope?.subjectId !== undefined
        ? z.uuidv7().parse(selected.scope.subjectId)
        : selected?.subjectId !== undefined
          ? z.uuidv7().parse(selected.subjectId)
          : undefined);
    const submission = headers.get('idempotency-key') ?? uuid();
    const audience = z
      .array(z.string())
      .parse(
        headers.has('x-audience')
          ? JSON.parse(headers.get('x-audience')!)
          : subjectId
            ? [subjectId]
            : [],
      );
    let ctx = makeContext({
      actor,
      consumer,
      subjectId,
      purpose: headers.get('x-purpose') ?? 'management',
      audience,
      operationId: submission,
      traceparent: headers.get('traceparent') ?? undefined,
    });
    const selectedAuthority = headers.get('x-delegation-authority');
    if (selectedAuthority)
      ctx = await authentication.selectDelegation(ctx, selectedAuthority);
    const operationId = createHash('sha256')
      .update(
        JSON.stringify([
          {
            consumer: ctx.consumer,
            actor: ctx.actor.kind === 'human' ? ctx.actor.id : null,
          },
          ctx.subjectId,
          request.url.split('?')[0],
          submission,
        ]),
      )
      .digest('hex');
    return CallContextSchema.parse({
      ...ctx,
      trace: { ...ctx.trace, operationId },
    });
  }
  function route<I extends z.ZodType, O extends z.ZodType>(
    method: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE',
    url: string,
    operationId: string,
    input: I,
    output: O,
    handler: (
      ctx: CallContext,
      input: z.output<I>,
      routeParams: RouteIds,
    ) => Promise<z.input<O>>,
  ) {
    const routeParamsSchema = z.object({
      ...(url.includes(':subjectId') ? { subjectId: z.uuidv7() } : {}),
      ...(url.includes(':id') ? { id: z.string().min(1) } : {}),
      ...(url.includes(':action') ? { action: z.string().min(1) } : {}),
    });
    server.route({
      method,
      url,
      schema: {
        operationId,
        security: [{ bearer: [] }],
        ...(url.includes(':') ? { params: routeParamsSchema } : {}),
        ...(method === 'GET' ? { querystring: input } : { body: input }),
        response: {
          200: output,
          400: ProblemSchema,
          401: ProblemSchema,
          403: ProblemSchema,
          404: ProblemSchema,
          409: ProblemSchema,
          422: ProblemSchema,
          429: ProblemSchema,
          503: ProblemSchema,
        },
      },
      handler: async (request, reply) => {
        const ctx = await context(request);
        return app().telemetry.span(ctx, `nous.${operationId}`, async (ctx) => {
          request.callContext = ctx;
          reply
            .header('x-operation-id', ctx.trace.operationId)
            .header('x-trace-id', ctx.trace.traceId);
          const data = input.parse(
            method === 'GET' ? request.query : request.body,
          );
          const result = await handler(ctx, data, request.params as RouteIds);
          request.log.info({
            event: 'http.operation_completed',
            operationId: ctx.trace.operationId,
            traceId: ctx.trace.traceId,
            subjectId: ctx.subjectId,
            actorKind: ctx.actor.kind,
            outcome: 'completed',
          });
          return result;
        });
      },
    });
  }
  const empty = z.object({}),
    unknown = z.unknown();
  route(
    'POST',
    '/v1/time/read',
    'time.read',
    TimeSelectionSchema.extend({}),
    CapturedTimeSchema,
    (ctx, input) => app().time.read(ctx, input),
  );
  route(
    'POST',
    '/v1/time/captures',
    'time.capture',
    TimeCaptureRequestSchema,
    TimeCaptureReceiptSchema,
    (ctx, { existingOnly, ...selection }) =>
      app().time.capture(ctx, {
        operationId: ctx.trace.operationId,
        selection,
        existingOnly,
      }),
  );
  route(
    'GET',
    `${S}/time/captures/:id`,
    'time.readCapture',
    empty,
    TimeCaptureReceiptSchema,
    (ctx, _input, p) =>
      app().time.readCapture(ctx, {
        product: 'nous',
        installationId: app().installationId,
        subjectId: ctx.subjectId,
        kind: 'TimeCapture',
        id: p.id!,
      }),
  );
  route(
    'GET',
    '/v1/identity',
    'identity.read',
    empty,
    AuthenticatedIdentitySchema,
    async (ctx) => ({ actor: ctx.actor, consumer: ctx.consumer }),
  );
  route(
    'POST',
    `${S}/sources/resolve`,
    'sources.resolve',
    SourcesResolveSchema,
    SourceResolutionSchema,
    async (ctx, input) => {
      const { resolvedRefs, roots, policies } =
        await app().knowledge.sources.resolve(ctx, input.refs);
      return { resolvedRefs, roots, policies };
    },
  );
  route(
    'GET',
    '/v1/status',
    'host.status',
    empty,
    HostStatusSchema,
    async (ctx) => {
      await app().authorize(ctx, 'host.status');
      return app().host.status();
    },
  );
  route(
    'POST',
    '/v1/host/drain',
    'host.drain',
    empty,
    z.object({ state: z.string(), acceptingWork: z.boolean() }),
    async (ctx) => {
      await app().authorize(ctx, 'host.drain');
      return app().host.drain();
    },
  );
  server.post(
    '/v1/host/stop',
    {
      schema: {
        operationId: 'host.stop',
        body: z.object({ mode: z.enum(['graceful', 'force']) }),
        response: { 202: z.object({ state: z.literal('stopping') }) },
      },
    },
    async (request, reply) => {
      const ctx = await context(request);
      await app().authorize(ctx, 'host.stop');
      if (!options.stop)
        throw new DomainError(
          'dependency_unavailable',
          '当前Host未注册停止入口',
        );
      const { mode } = request.body as { mode: 'graceful' | 'force' };
      reply.raw.once('finish', () => {
        void options.stop!(mode).catch(() => {});
      });
      return reply.code(202).send({ state: 'stopping' as const });
    },
  );
  route(
    'POST',
    '/v1/management-keys',
    'credentials.createManagementKey',
    z.object({ name: z.string().min(1).max(256) }),
    z.object({
      id: z.string(),
      key: z.string(),
      authorityRef: z.string(),
      delegationId: z.string(),
    }),
    (ctx, input) => app().credentials.createManagementKey(ctx, input.name),
  );
  server.post(
    `${S}/blobs`,
    {
      schema: {
        operationId: 'materials.uploadBlob',
        security: [{ bearer: [] }],
        consumes: ['application/octet-stream'],
        body: z.any().meta({ type: 'string', format: 'binary' }),
        params,
        response: {
          200: BlobResultSchema,
        },
      },
    },
    async (request, reply) => {
      const ctx = await context(request);
      const mediaType = z
        .string()
        .regex(/^[a-zA-Z0-9.+-]+\/[a-zA-Z0-9.+-]+$/)
        .parse(
          request.headers['x-content-media-type'] ?? 'application/octet-stream',
        );
      const result = await app().materials.putBlobStream(
        ctx,
        request.body as Readable,
        mediaType,
      );
      reply.header('x-operation-id', ctx.trace.operationId);
      return result;
    },
  );
  route('GET', '/v1/health', 'health.read', empty, unknown, async (ctx) =>
    app()
      .authorize(ctx, 'host.status')
      .then(() => app().host.doctor()),
  );
  route(
    'POST',
    '/v1/subjects',
    'subjects.create',
    SubjectCreateSchema,
    unknown,
    (ctx, input) => app().subjects.create(ctx, input),
  );
  route(
    'GET',
    '/v1/subjects',
    'subjects.list',
    empty,
    unknown,
    async (ctx) => ({ items: await app().subjects.list(ctx) }),
  );
  route('GET', S, 'subjects.read', empty, unknown, (ctx) =>
    app().subjects.read(ctx),
  );
  route(
    'POST',
    `${S}/bindings`,
    'subjects.bind',
    z.object({
      installationId: z.string(),
      subjectId: z.uuidv7(),
      connectionRef: z.string(),
    }),
    unknown,
    (ctx, input) => app().subjects.bind(ctx, input),
  );
  route(
    'POST',
    `${S}/materials/publish`,
    'materials.publish',
    MaterialPublishSchema,
    MaterialViewSchema,
    (ctx, input) => app().materials.publish(ctx, input),
  );
  route(
    'PATCH',
    `${S}/capabilities`,
    'subjects.capabilities',
    z.object({ capabilities: z.array(KnowledgeKindSchema) }),
    unknown,
    (ctx, input) => app().subjects.provision(ctx, input.capabilities),
  );
  route(
    'GET',
    `${S}/changes`,
    'subjects.changes',
    z.object({ after: z.string().default('0') }),
    unknown,
    async (ctx, input) => ({
      items: await app().subjects.changes(ctx, input.after),
    }),
  );
  route(
    'POST',
    `${S}/materials`,
    'materials.capture',
    MaterialCaptureSchema,
    MaterialViewSchema,
    (ctx, input) => app().materials.capture(ctx, input),
  );
  route(
    'GET',
    `${S}/materials/:id`,
    'materials.read',
    z.object({
      revision: z.coerce.number().int().positive().optional(),
      selector: SelectorQuerySchema.optional(),
    }),
    MaterialViewSchema,
    (ctx, input, p) =>
      app().materials.read(ctx, {
        ref: {
          product: 'nous',
          installationId: app().installationId,
          subjectId: ctx.subjectId,
          kind: 'Material',
          id: p.id!,
          revision: input.revision,
          selector: input.selector,
        },
      }),
  );
  route(
    'PATCH',
    `${S}/materials/:id/policy`,
    'policy.update',
    PolicySchema,
    z.object({ ref: RefSchema, policy: PolicySchema }),
    (ctx, input, p) =>
      app().materials.updatePolicy(ctx, {
        ref: {
          product: 'nous',
          installationId: app().installationId,
          subjectId: ctx.subjectId,
          kind: 'Material',
          id: p.id!,
        },
        policy: input,
      }),
  );
  route(
    'POST',
    `${S}/encounters`,
    'encounters.accept',
    EncounterAcceptSchema,
    EncounterViewSchema,
    (ctx, input) => app().encounters.accept(ctx, input),
  );
  route(
    'POST',
    `${S}/knowledge`,
    'knowledge.create',
    KnowledgeCreateSchema,
    KnowledgeViewSchema,
    (ctx, input) => app().knowledge.create(ctx, input),
  );
  route(
    'GET',
    `${S}/knowledge/:id`,
    'knowledge.read',
    KnowledgeReadSchema.omit({ id: true }),
    KnowledgeViewSchema,
    (ctx, input, p) => app().knowledge.read(ctx, { ...input, id: p.id! }),
  );
  route(
    'GET',
    `${S}/knowledge/:id/history`,
    'knowledge.history',
    KnowledgeHistorySchema.extend({
      limit: z.coerce.number().int().positive().max(200).default(100),
    }),
    KnowledgePageSchema,
    (ctx, input, p) => app().knowledge.history(ctx, p.id!, input),
  );
  route(
    'POST',
    `${S}/knowledge/:id/revisions`,
    'knowledge.revise',
    KnowledgeReviseSchema.omit({ id: true }),
    KnowledgeViewSchema,
    (ctx, input, p) => app().knowledge.revise(ctx, { ...input, id: p.id! }),
  );
  route(
    'POST',
    `${S}/knowledge/query`,
    'knowledge.query',
    QuerySchema,
    QueryResultSchema,
    (ctx, input) => app().knowledge.query(ctx, input),
  );
  route(
    'POST',
    `${S}/knowledge/expand`,
    'knowledge.expand',
    ExpandSchema,
    ExpansionResultSchema,
    (ctx, input) => app().knowledge.expand(ctx, input),
  );
  route(
    'POST',
    `${S}/knowledge/query-explanation`,
    'query.explain',
    z.union([z.string(), QuerySchema]),
    unknown,
    (ctx, input) => app().knowledge.explainQuery(ctx, input),
  );
  route(
    'POST',
    `${S}/context-views`,
    'contexts.prepare',
    ContextPrepareSchema,
    ContextViewSchema,
    (ctx, input) => app().contexts.prepare(ctx, input),
  );
  route(
    'POST',
    `${S}/work-contexts`,
    'contexts.createWork',
    WorkContextBodySchema,
    WorkContextSchema,
    (ctx, input) => app().contexts.createWork(ctx, input),
  );
  route(
    'GET',
    `${S}/work-contexts/:id`,
    'contexts.read',
    empty,
    WorkContextSchema,
    (ctx, _, p) => app().contexts.read(ctx, { id: p.id! }),
  );
  route(
    'PATCH',
    `${S}/work-contexts/:id`,
    'contexts.updateWork',
    z.object({
      expectedRevision: z.number().int(),
      body: WorkContextBodySchema,
    }),
    WorkContextSchema,
    (ctx, input, p) => app().contexts.updateWork(ctx, { ...input, id: p.id! }),
  );
  route(
    'POST',
    `${S}/feedback`,
    'cognition.feedback',
    FeedbackSchema,
    unknown,
    (ctx, input) => app().feedback(ctx, input),
  );
  route(
    'POST',
    `${S}/growth`,
    'growth.run',
    z.object({
      experienceRefs: z.array(RefSchema).optional(),
      formed: TimePointInputSchema.optional(),
      apply: z.boolean().optional(),
    }),
    unknown,
    (ctx, input) => app().growth.run(ctx, input),
  );
  route(
    'GET',
    `${S}/growth/:id`,
    'growth.inspect',
    empty,
    unknown,
    (ctx, _, p) => app().growth.inspect(ctx, { id: p.id! }),
  );
  route(
    'POST',
    `${S}/growth/:id/apply`,
    'growth.apply',
    z.object({ groupIds: z.array(z.string()).optional() }),
    unknown,
    (ctx, input, p) => app().growth.apply(ctx, { ...input, id: p.id! }),
  );
  route(
    'POST',
    `${S}/growth/:id/resolve`,
    'growth.resolve',
    GrowthResolveSchema.omit({ id: true }),
    unknown,
    (ctx, input, p) => app().growth.resolve(ctx, { ...input, id: p.id! }),
  );
  route(
    'POST',
    `${S}/seeds/preview`,
    'seeds.preview',
    z.object({ refs: z.array(RefSchema), instructions: z.string().optional() }),
    unknown,
    (ctx, input) => app().seeds.preview(ctx, input),
  );
  route(
    'POST',
    `${S}/seeds/apply`,
    'seeds.apply',
    SeedApplySchema,
    unknown,
    (ctx, input) => app().seeds.apply(ctx, input),
  );
  route(
    'POST',
    `${S}/episodes/resegment`,
    'episodes.organize',
    EpisodeOrganizeSchema,
    unknown,
    (ctx, input) => app().episodes.organize(ctx, input),
  );
  route(
    'POST',
    `${S}/journals`,
    'journals.write',
    JournalWriteSchema,
    KnowledgeViewSchema,
    (ctx, input) => app().journals.write(ctx, input),
  );
  route(
    'POST',
    `${S}/concepts/merge`,
    'concepts.merge',
    ConceptMergeSchema,
    unknown,
    (ctx, input) => app().concepts.merge(ctx, input),
  );
  route(
    'POST',
    `${S}/concepts/split`,
    'concepts.split',
    ConceptSplitSchema,
    unknown,
    (ctx, input) => app().concepts.split(ctx, input),
  );
  route(
    'POST',
    `${S}/motivation/:id/evaluations`,
    'motivation.evaluate',
    MotivationEvaluateSchema.omit({ id: true }),
    unknown,
    (ctx, input, p) => app().motivation.evaluate(ctx, { ...input, id: p.id! }),
  );
  route('GET', `${S}/persona`, 'persona.read', empty, unknown, (ctx) =>
    app().persona.read(ctx),
  );
  route(
    'POST',
    `${S}/resources`,
    'resources.register',
    ResourceBodySchema,
    unknown,
    (ctx, input) => app().resources.register(ctx, input),
  );
  route(
    'GET',
    `${S}/resources/:id`,
    'resources.inspect',
    empty,
    unknown,
    (ctx, _, p) => app().resources.inspect(ctx, { id: p.id! }),
  );
  route(
    'POST',
    `${S}/resources/:id/read`,
    'resources.read',
    z.object({ target: z.string() }),
    MaterialViewSchema,
    (ctx, input, p) => app().resources.read(ctx, { ...input, id: p.id! }),
  );
  route(
    'GET',
    '/v1/settings/schema',
    'settings.describe',
    SettingsDescribeSchema.extend({}),
    z.array(ModuleDescriptionSchema),
    (ctx, input) => app().settings.describe(ctx, input),
  );
  const settingQuery = SettingsReadSchema.extend({
    namespaces: z.preprocess(
      (value) => (typeof value === 'string' ? [value] : value),
      z.array(z.string().min(1)),
    ),
  });
  route(
    'GET',
    '/v1/settings',
    'settings.read',
    settingQuery,
    EffectiveSettingsViewSchema,
    (ctx, input) => app().settings.read(ctx, input),
  );
  route(
    'PATCH',
    '/v1/settings',
    'settings.update',
    SettingsChangeSchema,
    SettingsChangeResultSchema,
    (ctx, input) => app().settings.update(ctx, input),
  );
  route(
    'POST',
    '/v1/settings/reset',
    'settings.reset',
    SettingsResetSchema,
    SettingsChangeResultSchema,
    (ctx, input) => app().settings.reset(ctx, input),
  );
  route(
    'GET',
    '/v1/settings/history',
    'settings.history',
    SettingsHistorySchema.extend({
      limit: z.coerce.number().int().min(1).max(200).optional(),
    }),
    SettingsHistoryPageSchema,
    (ctx, input) => app().settings.history(ctx, input),
  );
  route(
    'POST',
    '/v1/settings/restore',
    'settings.restore',
    SettingsRestoreSchema,
    SettingsChangeResultSchema,
    (ctx, input) => app().settings.restore(ctx, input),
  );
  route(
    'GET',
    '/v1/settings/applied',
    'settings.applied',
    settingQuery,
    z.array(ConsumerAppliedViewSchema),
    (ctx, input) => app().settings.applied(ctx, input),
  );
  const backend = z.discriminatedUnion('kind', [
    z.strictObject({
      kind: z.literal('environment'),
      variable: z.string().regex(/^[A-Za-z_][A-Za-z0-9_]*$/),
    }),
    z.strictObject({
      kind: z.enum(['protected_file', 'os']),
      material: z.string().min(1).max(65536),
    }),
  ]);
  async function secretWrite<T>(
    value: z.infer<typeof backend>,
    use: (
      backend: import('@nous-wave/platform/credentials').SecretCreate['backend'],
    ) => Promise<T>,
  ) {
    if (value.kind === 'environment') return use(value);
    const bytes = Buffer.from(value.material, 'utf8');
    try {
      return await use({ kind: value.kind, material: bytes });
    } finally {
      bytes.fill(0);
    }
  }
  route(
    'POST',
    '/v1/secrets',
    'secrets.create',
    z.strictObject({
      purpose: z.string(),
      resourceRef: RefSchema,
      allowedConsumers: z.array(ConsumerSchema),
      backend,
    }),
    SecretStatusSchema,
    (ctx, input) =>
      secretWrite(input.backend, (backend) =>
        app().secrets.create(ctx, { ...input, backend }),
      ),
  );
  route(
    'GET',
    '/v1/secrets/:id',
    'secrets.status',
    empty,
    SecretStatusSchema,
    (ctx, _, p) => app().secrets.status(ctx, { ref: p.id! }),
  );
  route(
    'POST',
    '/v1/secrets/:id/rotate',
    'secrets.rotate',
    z.strictObject({
      expectedRevision: z.number().int().positive(),
      backend,
      allowedConsumers: z.array(ConsumerSchema).optional(),
    }),
    SecretStatusSchema,
    (ctx, input, p) =>
      secretWrite(input.backend, (backend) =>
        app().secrets.rotate(ctx, { ...input, ref: p.id!, backend }),
      ),
  );
  route(
    'POST',
    '/v1/secrets/:id/revoke',
    'secrets.revoke',
    z.strictObject({ expectedRevision: z.number().int().positive() }),
    SecretStatusSchema,
    (ctx, input, p) => app().secrets.revoke(ctx, { ...input, ref: p.id! }),
  );
  route(
    'POST',
    '/v1/delegations/children',
    'delegations.createChild',
    DelegationChildCreateSchema,
    DelegationChildViewSchema,
    (ctx, input) => app().credentials.delegations.createChild(ctx, input),
  );
  route(
    'POST',
    '/v1/delegations',
    'delegations.create',
    z.strictObject({
      name: z.string().min(1).max(256),
      scope: DelegationScopeSchema,
      principalKind: z.enum([
        'subject',
        'worker',
        'author',
        'human',
        'service',
      ]),
    }),
    unknown,
    (ctx, input) => app().credentials.delegations.create(ctx, input),
  );
  route(
    'GET',
    '/v1/delegations/:id',
    'delegations.read',
    empty,
    unknown,
    (ctx, _, p) => app().credentials.delegations.read(ctx, p.id!),
  );
  route(
    'PATCH',
    '/v1/delegations/:id',
    'delegations.update',
    z.strictObject({
      expectedRevision: z.number().int().positive(),
      scope: DelegationScopeSchema,
    }),
    unknown,
    (ctx, input, p) => app().credentials.delegations.update(ctx, p.id!, input),
  );
  route(
    'POST',
    '/v1/delegations/:id/revoke',
    'delegations.revoke',
    z.strictObject({ expectedRevision: z.number().int().positive() }),
    unknown,
    (ctx, input, p) => app().credentials.delegations.revoke(ctx, p.id!, input),
  );
  route(
    'GET',
    '/v1/network',
    'network.inspect',
    z.strictObject({
      scope: ConfigScopeSchema.optional(),
      profileId: z.string().optional(),
    }),
    unknown,
    (ctx, input) => app().network.inspect(ctx, input),
  );
  route(
    'POST',
    '/v1/network/probes',
    'network.probe',
    z.strictObject({
      resourceRef: RefSchema,
      kind: z.enum(['dns', 'tls', 'http']),
      url: z.url(),
    }),
    unknown,
    async (ctx, input) =>
      app().network.probe(ctx, {
        ...(await app().models.networkSelection(ctx, input.resourceRef)),
        kind: input.kind,
        url: input.url,
      }),
  );
  route(
    'GET',
    `${S}/context-views/:id`,
    'contexts.readView',
    empty,
    ContextViewSchema,
    (ctx, _input, p) => app().contexts.readView(ctx, { id: p.id! }),
  );
  route(
    'POST',
    '/v1/models/test',
    'models.test',
    ModelTestInputSchema,
    OperationRefSchema,
    (ctx, input) => app().testModel(ctx, input),
  );
  const cas = z.number().int().nonnegative().safe();
  for (const [name, definition] of [
    ['providers', ProviderDefinitionSchema],
    ['models', ModelRegistrationSchema],
  ] as const) {
    const owner = () =>
      name === 'providers' ? app().models.providers : app().models.resources;
    const path = name === 'providers' ? '/v1/model-providers' : '/v1/models';
    const entry = z.strictObject({ ref: ResourceRefSchema, definition });
    route('GET', path, `${name}.list`, empty, z.array(entry), (ctx) =>
      owner().list(ctx),
    );
    route(
      'GET',
      `${path}/:id`,
      `${name}.read`,
      z.strictObject({
        revision: z.coerce.number().int().positive().optional(),
      }),
      entry.extend({ snapshot: ConfigSnapshotSchema }),
      (ctx, input, p) => owner().read(ctx, { ...input, id: p.id! }),
    );
    route(
      'POST',
      path,
      `${name}.create`,
      z.strictObject({ definition, expectedRevision: cas }),
      SettingsChangeResultSchema,
      (ctx, input) => owner().create(ctx, input),
    );
    route(
      'PATCH',
      `${path}/:id`,
      `${name}.update`,
      z.strictObject({ definition, expectedRevision: cas }),
      SettingsChangeResultSchema,
      (ctx, input, p) => owner().update(ctx, { ...input, id: p.id! }),
    );
    route(
      'DELETE',
      `${path}/:id`,
      `${name}.delete`,
      z.strictObject({ expectedRevision: cas }),
      SettingsChangeResultSchema,
      (ctx, input, p) => owner().delete(ctx, { ...input, id: p.id! }),
    );
  }
  route(
    'GET',
    '/v1/roles',
    'models.roles',
    z.strictObject({ scope: ConfigScopeSchema.optional() }),
    unknown,
    (ctx, input) => app().models.describeRoles(ctx, input.scope),
  );
  route(
    'POST',
    `${S}/models/prepare`,
    'models.prepare',
    z.strictObject({ roleId: z.string().min(1) }),
    InvocationSnapshotSchema,
    (ctx, input) => app().models.prepare(ctx, input.roleId),
  );
  route(
    'POST',
    `${S}/model-invocations/prepare`,
    'modelInvocations.prepare',
    NativeInvocationPrepareSchema,
    ModelCallViewSchema,
    (ctx, input) => app().nativeModels.prepare(ctx, input),
  );
  route(
    'POST',
    `${S}/model-invocations/:id/dispatch`,
    'modelInvocations.dispatch',
    z.strictObject({ observed: TimeCaptureReceiptSchema.optional() }),
    ModelCallViewSchema,
    (ctx, input, p) =>
      app().nativeModels.dispatch(ctx, { ...input, invocationId: p.id! }),
  );
  route(
    'POST',
    `${S}/model-invocations/:id/completion`,
    'modelInvocations.complete',
    NativeInvocationCompletionSchema,
    ModelCallViewSchema,
    (ctx, input, p) => {
      if (input.invocationId !== p.id)
        throw problem(
          'models.invocation_mismatch',
          'conflict',
          '路径与回执的模型调用身份不一致',
        );
      return app().nativeModels.complete(ctx, input);
    },
  );
  route(
    'GET',
    '/v1/model-invocations',
    'models.invocations.list',
    ModelInvocationFilterSchema.extend({
      limit: z.coerce.number().int().min(1).max(200).optional(),
    }),
    ModelInvocationPageSchema,
    (ctx, input) => app().models.invocations.list(ctx, input),
  );
  route(
    'GET',
    '/v1/model-invocations/:id',
    'models.invocations.read',
    z.strictObject({ subjectId: z.uuidv7().optional() }),
    ModelCallViewSchema,
    (ctx, _input, p) =>
      app().models.invocations.read(ctx, { invocationId: p.id! }),
  );
  route(
    'POST',
    '/v1/model-invocations/reconcile',
    'models.reconcile',
    ModelReconcileSchema.extend({ subjectId: z.uuidv7().optional() }),
    ModelCallViewSchema,
    (ctx, { subjectId: _subjectId, ...input }) =>
      app().models.reconcile(ctx, input),
  );
  route(
    'GET',
    '/v1/costs',
    'costs.read',
    ModelInvocationFilterSchema.extend({
      limit: z.coerce.number().int().min(1).max(200).optional(),
    }),
    ModelInvocationPageSchema,
    (ctx, input) => app().models.costs.read(ctx, input),
  );

  route(
    'GET',
    '/v1/prompts/catalog',
    'prompts.describe',
    PromptDescribeSchema.extend({}),
    z.array(PromptDescriptionSchema),
    (ctx, input) => app().prompts.describe(ctx, input),
  );
  route(
    'GET',
    '/v1/prompts',
    'prompts.list',
    PromptListSchema.extend({
      limit: z.coerce.number().int().min(1).max(200).optional(),
    }),
    PromptListPageSchema,
    (ctx, input) => app().prompts.list(ctx, input),
  );
  route(
    'GET',
    '/v1/prompts/read',
    'prompts.read',
    PromptReadSchema.extend({
      revision: z.coerce.number().int().positive().optional(),
    }),
    PromptReadResultSchema,
    (ctx, input) => app().prompts.read(ctx, input),
  );
  route(
    'GET',
    '/v1/prompts/history',
    'prompts.history',
    PromptHistorySchema.extend({
      limit: z.coerce.number().int().min(1).max(200).optional(),
    }),
    PromptHistoryPageSchema,
    (ctx, input) => app().prompts.history(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts',
    'prompts.save',
    PromptSaveSchema,
    PromptChangeResultSchema,
    (ctx, input) => app().prompts.save(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts/activate',
    'prompts.activate',
    PromptActivateSchema,
    PromptChangeResultSchema,
    (ctx, input) => app().prompts.activate(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts/reset',
    'prompts.reset',
    PromptResetSchema,
    PromptChangeResultSchema,
    (ctx, input) => app().prompts.reset(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts/preview',
    'prompts.preview',
    PromptPreviewSchema,
    PromptPreviewResultSchema,
    (ctx, input) => app().prompts.preview(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts/snapshot',
    'prompts.snapshot',
    PromptSnapshotRequestSchema,
    PromptSnapshotSchema,
    (ctx, input) => app().prompts.snapshot(ctx, input),
  );
  route(
    'POST',
    '/v1/prompts/render',
    'prompts.render',
    PromptRenderRequestSchema,
    PromptRenderResultSchema,
    (ctx, input) => app().prompts.render(ctx, input),
  );
  route(
    'GET',
    '/v1/operations/:id',
    'operations.inspect',
    OperationControlSchema,
    OperationViewSchema,
    (ctx, _, p) => app().operations.inspect(ctx, p.id!),
  );
  route(
    'GET',
    '/v1/operations/:id/trace',
    'operations.trace',
    OperationControlSchema,
    OperationTraceSchema,
    (ctx, _, p) => app().operations.trace(ctx, p.id!),
  );
  route(
    'POST',
    '/v1/operations/:id/cancel',
    'operations.cancel',
    OperationControlSchema,
    OperationViewSchema,
    (ctx, _input, p) => app().operations.cancel(ctx, p.id!),
  );
  route(
    'POST',
    '/v1/operations/:id/resume',
    'operations.resume',
    OperationControlSchema,
    OperationViewSchema,
    (ctx, _input, p) => app().operations.resume(ctx, p.id!),
  );
  server.get('/openapi.json', { schema: { hide: true } }, async () =>
    server.swagger(),
  );
  return server;
}
