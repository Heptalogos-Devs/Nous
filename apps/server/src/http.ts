import Fastify from 'fastify';
import swagger from '@fastify/swagger';
import swaggerUI from '@fastify/swagger-ui';
import {
  validatorCompiler,
  serializerCompiler,
  createJsonSchemaTransform,
  createJsonSchemaTransformObject,
} from 'fastify-type-provider-zod';
import { z } from 'zod';
import { createHash } from 'node:crypto';
import { v7 as uuid } from 'uuid';
import type { Readable } from 'node:stream';
import {
  ActorSchema,
  CallContextSchema,
  ProblemSchema,
  RefSchema,
  TimePointSchema,
  PolicySchema,
} from '@nous-wave/contracts/common';
import {
  EpisodeOrganizeSchema,
  JournalWriteSchema,
  ConceptMergeSchema,
  ConceptSplitSchema,
  MotivationEvaluateSchema,
  KnowledgeCreateSchema,
  KnowledgeReviseSchema,
  KnowledgeViewSchema,
  KnowledgeReadSchema,
  KnowledgeKindSchema,
  KnowledgeHistorySchema,
  KnowledgePageSchema,
} from '@nous-wave/contracts/knowledge';
import {
  MaterialCaptureSchema,
  MaterialViewSchema,
  MaterialPublishSchema,
  EncounterAcceptSchema,
  EncounterViewSchema,
} from '@nous-wave/contracts/materials';
import {
  ContextPrepareSchema,
  ContextViewSchema,
  QuerySchema,
  QueryResultSchema,
  ExpandSchema,
  ExpansionResultSchema,
  WorkContextBodySchema,
  WorkContextSchema,
  FeedbackSchema,
  ResourceBodySchema,
  GrowthResolveSchema,
  SeedApplySchema,
} from '@nous-wave/contracts/cognition';
import {
  InvocationPrepareSchema,
  InvocationCompletionSchema,
  ModelResourceSchema,
  SettingsScopeSchema,
  SettingsUpdateSchema,
  SettingsResetSchema,
  SettingsRestoreSchema,
} from '@nous-wave/contracts/management';
import { SubjectCreateSchema } from '@nous-wave/contracts/identity';
import { DomainError } from '@nous-wave/nous';
import { span } from '@nous-wave/nous';
import { makeContext } from '@nous-wave/nous';
import type { Nous } from '@nous-wave/nous';
import type { CallContext } from '@nous-wave/contracts/common';

export async function createHttp(
  application: Nous | undefined = undefined,
  options: { baseURL?: string; authentication?: boolean } = {},
) {
  const app = () => {
    if (!application)
      throw new Error('Schema-only server cannot execute business operations');
    return application;
  };
  const server = Fastify({
    logger: { redact: ['req.headers.authorization', 'req.headers.cookie'] },
  });
  server.addContentTypeParser('*', (_request, payload, done) =>
    done(null, payload),
  );
  server.setValidatorCompiler(validatorCompiler);
  server.setSerializerCompiler(serializerCompiler);
  await server.register(swagger, {
    openapi: {
      openapi: '3.0.3',
      info: { title: 'Nous Wave', version: '0.1.0' },
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
      : await app().authentication(options.baseURL);
  if (authentication)
    server.route({
      method: ['GET', 'POST'],
      url: '/api/auth/*',
      schema: { hide: true },
      handler: async (request, reply) => {
        const path = request.url.split('?')[0]!;
        if (path.includes('/sign-up') || path.includes('/api-key/'))
          return reply
            .code(403)
            .send({ code: 'forbidden', title: '此操作使用正式管理入口' });
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
  server.setErrorHandler((error, request, reply) => {
    if (error instanceof DomainError)
      return reply.code(error.status).send(error.problem());
    if(error instanceof Error&&'code' in error&&ProblemSchema.shape.code.safeParse(error.code).success){
      const restored=new DomainError(ProblemSchema.shape.code.parse(error.code),error.message,'details' in error?error.details as {operationId?:string;currentRevision?:number}:{});
      return reply.code(restored.status).send(restored.problem());
    }
    if (
      (error instanceof Error && 'validation' in error) ||
      error instanceof z.ZodError
    )
      return reply
        .code(400)
        .send({
          type: 'urn:nous:problem:invalid_input',
          title: '请求字段无效',
          status: 400,
          code: 'invalid_input',
          detail: error instanceof Error ? error.message : String(error),
        });
    request.log.error({ err: error }, 'operation.failed');
    return reply
      .code(500)
      .send({
        type: 'urn:nous:problem:dependency_unavailable',
        title: '操作执行失败',
        status: 500,
        code: 'dependency_unavailable',
      });
  });
  const S = '/v1/subjects/:subjectId';
  const params = z.object({ subjectId: z.uuidv7() });
  type RouteIds = { subjectId?: string; id?: string; action?: string };
  async function context(request: {
    headers: Record<string, string | string[] | undefined>;
    params: unknown;
    url: string;
    method?:string;
    query?:unknown;
    body?:unknown;
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
    const actor = await authentication.actor(headers);
    const selected=(request.method==='GET'?request.query:request.body) as {subjectId?:unknown}|undefined;
    const subjectId = (request.params as { subjectId?: string })?.subjectId ?? (selected?.subjectId===undefined?'':z.uuidv7().parse(selected.subjectId));
    const submission = headers.get('idempotency-key') ?? uuid();
    const operationId = createHash('sha256')
      .update(
        JSON.stringify([
          actor.id,
          subjectId,
          request.url.split('?')[0],
          submission,
        ]),
      )
      .digest('hex');
    const audience = z
      .array(z.string())
      .parse(
        headers.has('x-audience')
          ? JSON.parse(headers.get('x-audience')!)
          : [subjectId],
      );
    return CallContextSchema.parse(
      makeContext(
        actor,
        subjectId,
        headers.get('x-purpose') ?? 'management',
        audience,
        operationId,
        headers.get('traceparent') ?? undefined,
      ),
    );
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
        return span(
          `nous.${operationId}`,
          typeof request.headers.traceparent==='string'
            ? { traceparent: request.headers.traceparent }
            : {},
          async () => {
            const ctx = await context(request);
            reply.header('x-operation-id',`${operationId}:${ctx.traceContext.operationId}`);
            const data=input.parse(method==='GET'?request.query:request.body);
            const result = await handler(ctx, data, request.params as RouteIds);
            request.log.info({
              event: operationId,
              operationId: ctx.traceContext.operationId,
              traceId: ctx.traceContext.traceId,
              subjectId: ctx.subjectId,
              actorKind: ctx.actor.kind,
              outcome: 'completed',
            });
            return result;
          },
        );
      },
    });
  }
  const empty = z.object({}),
    unknown = z.unknown();
  route(
    'POST',
    '/v1/access-keys',
    'credentials.issue',
    z.object({ name: z.string(), actor: ActorSchema }),
    z.object({ id: z.string(), key: z.string() }),
    (ctx, input) => app().credentials.issue(ctx, input),
  );
  server.post(
    `${S}/blobs`,
    {
      schema: {
        operationId: 'materials.uploadBlob',
        consumes:['application/octet-stream'],
        body:z.any().meta({type:'string',format:'binary'}),
        params,
        response: {
          200: z.object({
            ref: RefSchema,
            digest: z.string(),
            bytes: z.number().int().nonnegative(),
          }),
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
      reply.header(
        'x-operation-id',
        `materials.acceptBlob:${ctx.traceContext.operationId}`,
      );
      return result;
    },
  );
  route('GET', '/v1/health', 'health.read', empty, unknown, async () =>
    app().doctor(),
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
    (ctx,input)=>app().materials.publish(ctx,input),
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
    z.object({ revision: z.coerce.number().optional() }),
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
    KnowledgeHistorySchema.extend({limit:z.coerce.number().int().positive().max(200).default(100)}),
    KnowledgePageSchema,
    (ctx, input, p) => app().knowledge.history(ctx, p.id!,input),
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
      formed: TimePointSchema.optional(),
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
  route('POST',`${S}/growth/:id/resolve`,'growth.resolve',GrowthResolveSchema.omit({id:true}),unknown,(ctx,input,p)=>app().growth.resolve(ctx,{...input,id:p.id!}));
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
    empty,
    unknown,
    async () => app().settings.describe(),
  );
  route('GET','/v1/settings','settings.read',SettingsScopeSchema,unknown,(ctx,input)=>app().settings.read(ctx,input.role));
  route('PATCH','/v1/settings','settings.update',SettingsUpdateSchema,unknown,(ctx,input)=>app().settings.update(ctx,input));
  route('POST','/v1/settings/reset','settings.reset',SettingsResetSchema,unknown,(ctx,input)=>app().settings.reset(ctx,input));
  route('GET','/v1/settings/history','settings.history',SettingsScopeSchema,unknown,(ctx,input)=>app().settings.history(ctx,input));
  route('POST','/v1/settings/restore','settings.restore',SettingsRestoreSchema,unknown,(ctx,input)=>app().settings.restore(ctx,input));
  for(const method of ['PUT','GET','DELETE'] as const)route(method,'/v1/secrets/:id',`secrets.${method==='PUT'?'set':method==='GET'?'status':'delete'}`,method==='PUT'?z.object({value:z.string().min(1)}):empty,unknown,(ctx,input,p)=>method==='PUT'?app().secrets.set(ctx,p.id!,('value' in input?input.value:'') as string):method==='GET'?app().secrets.status(ctx,p.id!):app().secrets.delete(ctx,p.id!));
  route(
    'POST',
    '/v1/models',
    'models.register',
    ModelResourceSchema,
    ModelResourceSchema,
    (ctx, input) => app().models.register(ctx, input),
  );
  route(
    'GET',
    '/v1/models',
    'models.list',
    empty,
    z.object({ items: z.array(ModelResourceSchema) }),
    async (ctx) => ({ items: await app().models.list(ctx) }),
  );
  route(
    'POST',
    `${S}/model-invocations/prepare`,
    'modelInvocations.prepare',
    InvocationPrepareSchema,
    unknown,
    (ctx, input) => app().invocations.prepare(ctx, input),
  );
  route(
    'POST',
    `${S}/model-invocations/:id/dispatch`,
    'modelInvocations.dispatch',
    empty,
    unknown,
    (ctx, _, p) => app().invocations.dispatch(ctx, p.id!),
  );
  route(
    'POST',
    `${S}/model-invocations/:id/completion`,
    'modelInvocations.complete',
    InvocationCompletionSchema,
    unknown,
    (ctx, input, p) => app().invocations.complete(ctx, p.id!, input),
  );
  route('GET', '/v1/costs', 'costs.read', z.object({subjectId:z.uuidv7()}), unknown, async (ctx) => ({
    items: await app().costs.read(ctx),
  }));
  route(
    'GET',
    '/v1/operations/:id',
    'operations.inspect',
    z.object({subjectId:z.uuidv7().optional()}),
    unknown,
    (ctx, _, p) => app().operations.inspect(ctx, p.id!),
  );
  route('GET','/v1/operations/:id/trace','operations.trace',z.object({subjectId:z.uuidv7().optional()}),unknown,(ctx,_,p)=>app().operations.trace(ctx,p.id!));
  server.get('/openapi.json', { schema: { hide: true } }, async () =>
    server.swagger(),
  );
  return server;
}
