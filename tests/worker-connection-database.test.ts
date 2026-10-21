import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtemp, rm, mkdir } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import test from 'node:test';
import { Pool } from 'pg';
import { createBootstrap } from '@nous-wave/platform/bootstrap';
import { readReal, addMicros } from '@nous-wave/platform/time';
import {
  DelegationChildCreateSchema,
  DelegationScopeSchema,
} from '@nous-wave/contracts/credentials';
import {
  Configuration,
  IdentityApi, DelegationsApi, MaterialsApi, SourcesApi, ContextsApi,
  problemMiddleware,
  contextHeaders,
} from '@nous-wave/client';
import { createNous } from '../packages/nous/src/index.js';
import { createHttp } from '../apps/server/src/http.js';
import { makeContext } from '../packages/nous/src/platform/context.js';

test('Host经正式SDK选择真实Worker子委派；私有来源与撤权仍由原owner执行', {
  skip: !process.env.NOUS_TEST_DATABASE_URL,
}, async () => {
  const name = `nous_worker_${randomBytes(6).toString('hex')}`,
    maintenance = new Pool({
      connectionString: process.env.NOUS_TEST_DATABASE_URL,
      max: 1,
    });
  await maintenance.query(`create database ${name}`);
  const url = new URL(process.env.NOUS_TEST_DATABASE_URL!);
  url.pathname = `/${name}`;
  const root = resolve('../temp/tests');
  await mkdir(root, { recursive: true });
  const directory = await mkdtemp(join(root, 'worker-connection-'));
  const bootstrap = await createBootstrap({
    product: 'nous',
    instanceHome: directory,
    programRoot: process.cwd(),
  }).init({
    database: { mode: 'external', urlRef: 'test-database', maxConnections: 1 },
  });
  const app = await createNous({
    bootstrap,
    databaseUrl: url.toString(),
    authSecret: randomBytes(32).toString('hex'),
  });
  const server = await createHttp(app),
    basePath = await server.listen({ host: '127.0.0.1', port: 0 });
  try {
    const auth = app.authentication,
      user = await auth.bootstrapAdministrator({
        email: 'worker-connection@example.test',
        password: randomBytes(24).toString('base64url'),
        name: 'Worker connection',
      });
    const administrator = await auth.createManagementKey(user.userId, {
      name: 'connection owner',
      scope: app.managementScope(),
    });
    const subjectId = '019a05b0-0000-7000-8000-000000000001',
      taskId = '019a05b0-0000-7000-8000-000000000002';
    const principal = await auth.authenticate(
      new Headers({ authorization: `Bearer ${administrator.key}` }),
    );
    const ctx = makeContext({
      ...principal,
      subjectId,
      purpose: 'worker',
      audience: [subjectId],
    });
    await app.subjects.create(ctx, {
      id: subjectId,
      displayName: 'Worker连接场景',
      capabilities: ['Memory'],
    });
    const operations = [
      'subjects.read',
      'settings.read',
      'materials.capture',
      'materials.read',
      'materials.publish',
      'knowledge.read',
      'contexts.prepare',
      'contexts.read',
      'encounters.accept',
      'encounters.read',
    ];
    const expiresAt = {
      domainId: 'real',
      at: addMicros(readReal().at, 120_000_000n),
    };
    const host = await auth.delegations.create(ctx, {
      name: 'Hept Worker连接Host',
      principalKind: 'service',
      scope: DelegationScopeSchema.parse({
        plane: 'subject',
        operations: [
          ...operations,
          'delegations.create',
          'delegations.select',
          'delegations.read',
          'delegations.revoke',
        ],
        subjectIds: [subjectId],
        consumer: {
          kind: 'worker',
          host: 'hept-worker-connection',
          id: subjectId,
        },
        purposes: ['worker'],
        audiences: [subjectId],
        targets: [],
        inputRefs: [],
        expiresAt,
        mayDelegate: true,
      }),
    });
    const consumer = {
      kind: 'worker' as const,
      host: 'openclaw-worker-connection',
      id: taskId,
    };
    const request = DelegationChildCreateSchema.parse({
      name: 'Host签发的有期Task范围',
      principalKind: 'worker',
      scope: {
        plane: 'subject',
        operations,
        subjectIds: [subjectId],
        consumer,
        purposes: ['worker'],
        audiences: [subjectId],
        targets: [],
        inputRefs: [],
        taskRef: {
          product: 'hept',
          installationId: 'hept-worker-connection',
          subjectId,
          kind: 'task',
          id: taskId,
        },
        nativeBinding: {
          runtimeId: consumer.host,
          sessionKey: `agent:worker:task:${taskId}`,
          rootTaskId: taskId,
        },
        expiresAt,
        mayDelegate: false,
      },
    });
    const api = <T>(
      Api: new (configuration: Configuration) => T,
      token: string,
      submissionId: string,
      delegationAuthority?: string,
    ) =>
      new Api(
        new Configuration({
          basePath,
          accessToken: token,
          middleware: [problemMiddleware],
          headers: contextHeaders({
            submissionId,
            purpose: 'worker',
            audience: [subjectId],
            delegationAuthority,
          }),
        }),
      );
    const issuer = api(DelegationsApi,host.key, 'Task-child');
    const [first, concurrent] = await Promise.all([
      issuer.delegationsCreateChild({
        delegationChildCreateBodyInput: request,
      }),
      issuer.delegationsCreateChild({
        delegationChildCreateBodyInput: request,
      }),
    ]);
    assert.equal(first.authorityRef, concurrent.authorityRef);
    assert.equal('key' in first, false);
    assert.equal(
      (
        await issuer.delegationsCreateChild({
          delegationChildCreateBodyInput: request,
        })
      ).authorityRef,
      first.authorityRef,
      '丢确认重读不签发第二个active子委派',
    );
    await assert.rejects(
      issuer.delegationsCreateChild({
        delegationChildCreateBodyInput: { ...request, name: '同提交不同请求' },
      }),
      (error) =>
        (error as { problem?: { code: string } }).problem?.code ===
        'credentials.submission_conflict',
    );
    const selected = <T>(Api: new (configuration: Configuration) => T) => api(Api,host.key, 'private-input', first.authorityRef);
    assert.deepEqual((await selected(IdentityApi).identityRead()).consumer, consumer);
    const draft = await selected(MaterialsApi).materialsCapture({
      subjectId,
      materialCaptureInput: {
        kind: 'worker-input',
        mediaType: 'text/plain',
        text: '这一段只有实际Task Worker范围可以取得。',
        source: { roots: [] },
      },
    });
    assert.equal(
      (
        await selected(SourcesApi).sourcesResolve({
          subjectId,
          sourcesResolveBodyInput: { refs: [draft.ref] },
        })
      ).resolvedRefs.length,
      1,
    );
    const imageBytes=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR1cAAAAASUVORK5CYII=','base64');
    const imageBlob=await api(MaterialsApi,host.key,'worker-image-blob',first.authorityRef).materialsUploadBlob({subjectId,body:new Blob([imageBytes],{type:'image/png'})},async({init})=>{const headers=new Headers(init.headers);headers.set('x-content-media-type','image/png');return {...init,headers};});
    assert.equal(imageBlob.mediaType,'image/png');
    const image=await api(MaterialsApi,host.key,'worker-image-material',first.authorityRef).materialsCapture({subjectId,materialCaptureInput:{kind:'native_image_input',mediaType:'image/png',blobRef:imageBlob.ref,source:{roots:[]}}});
    const model={resourceId:'native-fixture',modelId:'native-fixture',contextLimit:2048},budget={outputTokens:64},observed=readReal();
    const imageView=await api(ContextsApi,host.key,'worker-image-context',first.authorityRef).contextsPrepare({subjectId,contextPrepareInput:{consumer,model,budget,observed,query:'',latestMaterials:[{ref:image.ref,text:'This label is not the image representation',selector:{kind:'image',x:0,y:0,width:1,height:1},required:true}]}});
    assert.equal(imageView.inputPortions.length,1);
    assert.deepEqual(imageView.inputPortions[0]!.selector,{kind:'image',x:0,y:0,width:1,height:1});
    assert.equal(imageView.inputPortions[0]!.text,'');
    assert.equal(imageView.blocks.length,0);
    const node=await api(MaterialsApi,host.key,'worker-node-material',first.authorityRef).materialsCapture({subjectId,materialCaptureInput:{kind:'native_model_output',mediaType:'application/json',text:JSON.stringify({content:[{type:'text',text:'Selected original text'}]}),source:{inputs:[draft.ref]}}});
    const nodeView=await api(ContextsApi,host.key,'worker-node-context',first.authorityRef).contextsPrepare({subjectId,contextPrepareInput:{consumer,model,budget,observed,query:'',latestMaterials:[{ref:node.ref,text:'',selector:{kind:'node',pointer:'/content/0/text'},required:true}]}});
    assert.equal(nodeView.inputPortions[0]!.text,'Selected original text');
    assert.deepEqual(nodeView.inputPortions[0]!.selector,{kind:'node',pointer:'/content/0/text'});
    await assert.rejects(
      api(SourcesApi,host.key,'host-source').sourcesResolve({
        subjectId,
        sourcesResolveBodyInput: { refs: [draft.ref] },
      }),
    );
    const subjectBridge = await auth.delegations.create(ctx, {
      name: '真实Subject连接',
      principalKind: 'service',
      scope: DelegationScopeSchema.parse({
        ...request.scope,
        consumer: { kind: 'subject', host: 'nous', id: subjectId },
        taskRef: undefined,
        nativeBinding: undefined,
      }),
    });
    await assert.rejects(
      api(SourcesApi,subjectBridge.key, 'subject-read').sourcesResolve({
        subjectId,
        sourcesResolveBodyInput: { refs: [draft.ref] },
      }),
    );
    await assert.rejects(
      api(IdentityApi,subjectBridge.key, 'wrong-parent', first.authorityRef).identityRead(),
    );
    await assert.rejects(
      api(IdentityApi,
        administrator.key,
        'human-selection',
        first.authorityRef,
      ).identityRead(),
    );
    const parentCtx = makeContext({
      ...(await auth.authenticate(
        new Headers({ authorization: `Bearer ${host.key}` }),
      )),
      purpose: 'worker',
      audience: [subjectId],
    });
    await assert.rejects(
      auth.delegations.create(parentCtx, {
        name: '相同Consumer不改变principal限制',
        principalKind: 'human',
        scope: DelegationScopeSchema.parse({
          ...request.scope,
          consumer: parentCtx.consumer,
          taskRef: undefined,
          nativeBinding: undefined,
        }),
      }),
    );
    await issuer.delegationsRevoke({
      id: first.delegationId,
      secretsRevokeRequest: { expectedRevision: 1 },
    });
    await assert.rejects(
      selected(SourcesApi).sourcesResolve({
        subjectId,
        sourcesResolveBodyInput: { refs: [draft.ref] },
      }),
    );
  } finally {
    await server.close();
    await app.host.stop();
    await maintenance.query(`drop database ${name}`);
    await maintenance.end();
    await rm(directory, { recursive: true, force: true });
  }
});
