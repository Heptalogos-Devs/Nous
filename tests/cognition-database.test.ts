import { roleCatalog } from '../packages/nous/src/models/catalog.js';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { randomBytes } from 'node:crypto';
import { mkdtemp, rm, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import test from 'node:test';
import { type Consumer } from '@nous-wave/contracts/common';
import { parseInstant } from '@nous-wave/platform/time';
import { createBootstrap } from '@nous-wave/platform/bootstrap';
import { createNous } from '../packages/nous/src/index.js';
import { isAccessDenied } from '../packages/nous/src/platform/security.js';
import { makeContext } from '../packages/nous/src/platform/context.js';

test('空数据库贯通收件资料、实际接触、SDK成长、费用与后续召回', {
  skip: !process.env.NOUS_TEST_DATABASE_URL,
}, async () => {
  const suffix = randomBytes(6).toString('hex');
  const subjectId = `019a05b0-0000-7000-8000-${suffix}`;
  const tempRoot = fileURLToPath(new URL('../../temp/', import.meta.url));
  await mkdir(tempRoot, { recursive: true });
  const directory = await mkdtemp(join(tempRoot, 'nous-cognition-'));
  let providerCalls = 0;
  let invalidStructured = false;
  let proposedGroups: unknown[] | undefined;
  const provider = createServer(async (request, response) => {
    const bytes: Buffer[] = [];
    for await (const chunk of request) bytes.push(Buffer.from(chunk));
    const body = JSON.parse(Buffer.concat(bytes).toString('utf8')) as {
      model: string;
    };
    if (body.model !== 'subject-reader')
      assert.equal(request.headers.authorization, 'Bearer local-fixture');
    providerCalls++;
    if (body.model === 'growth-json' && providerCalls === 2)
      assert.ok(
        JSON.stringify(body).includes('青柠咖啡馆'),
        '实际模型输入必须包含被采用的资料正文',
      );
    const content =
      body.model === 'subject-reader'
        ? '小林说周六下午去青柠咖啡馆。'
        : invalidStructured
          ? '这次实际模型输出不是 JSON。'
          : JSON.stringify({
              groups: proposedGroups ?? [
                {
                  id: 'coffee',
                  reason: '记住实际读过的约定',
                  changes: [
                    {
                      reason: '交往中明确告知',
                      kind: 'Memory',
                      body: {
                        kind: 'Memory',
                        content: '小林和我约好周六下午去青柠咖啡馆。',
                        role: 'reported',
                        entityRefs: [],
                        conceptRefs: [],
                        tags: ['咖啡'],
                      },
                      sourceIds: ['s1'],
                      formed: {
                        domainId: 'real',
                        at: '2026-10-20T08:01:00.000000Z',
                      },
                      formation: 'reflection',
                      stance: 'accepted',
                      confidence: 0.9,
                    },
                  ],
                },
              ],
            });
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(
      JSON.stringify({
        id: `fixture-${providerCalls}`,
        object: 'chat.completion',
        created: Math.floor(Date.now() / 1000),
        model: body.model,
        choices: [
          {
            index: 0,
            message: { role: 'assistant', content },
            finish_reason: 'stop',
          },
        ],
        usage: { prompt_tokens: 300, completion_tokens: 90, total_tokens: 390 },
      }),
    );
  });
  await new Promise<void>((resolve) =>
    provider.listen(0, '127.0.0.1', resolve),
  );
  const address = provider.address();
  assert.ok(address && typeof address !== 'string');
  const baseURL = `http://127.0.0.1:${address.port}/v1`;
  const bootstrap = await createBootstrap({
    product: 'nous',
    instanceHome: directory,
    programRoot: process.cwd(),
  }).init();
  const app = await createNous({
    bootstrap,
    authSecret: randomBytes(32).toString('hex'),
    databaseUrl: process.env.NOUS_TEST_DATABASE_URL!,
  });
  const authentication = app.authentication;
  const administrator = await authentication.bootstrapAdministrator({
    email: `cognition-${suffix}@example.test`,
    password: randomBytes(24).toString('base64url'),
    name: '认知场景管理员',
  });
  const operations = [...app.managementScope().operations];
  const expiresAt = {
    domainId: 'real',
    at: parseInstant(new Date(Date.now() + 3600000).toISOString()),
  };
  const key = await authentication.createManagementKey(administrator.userId, {
    name: '认知场景管理',
    scope: {
      plane: 'system',
      operations,
      subjectIds: [],
      consumer: { kind: 'maintenance', host: 'nous', id: app.installationId },
      purposes: ['conversation'],
      audiences: [subjectId],
      targets: [],
      inputRefs: [],
      expiresAt,
      mayDelegate: true,
    },
  });
  const administrative = await authentication.authenticate(
    new Headers({ 'x-api-key': key.key }),
  );
  const op = () =>
    makeContext({
      ...administrative,
      subjectId,
      purpose: 'conversation',
      audience: [subjectId],
    });
  async function delegate(consumer: Consumer, allowed: string[]) {
    const issued = await authentication.delegations.create(op(), {
      name: `场景-${consumer.id.slice(0, 16)}`,
      principalKind: consumer.kind === 'subject' ? 'subject' : 'worker',
      scope: {
        plane: 'subject',
        operations: allowed,
        subjectIds: [subjectId],
        consumer,
        purposes: ['conversation'],
        audiences: [subjectId],
        targets: [],
        inputRefs: [],
        expiresAt,
        mayDelegate: false,
      },
    });
    const principal = await authentication.authenticate(
      new Headers({ 'x-api-key': issued.key }),
    );
    return () =>
      makeContext({
        ...principal,
        subjectId,
        purpose: 'conversation',
        audience: [subjectId],
      });
  }
  let ctx: ReturnType<typeof makeContext>;
  const observed = { domainId: 'real', at: '2026-10-20T08:00:00.000000Z' };
  try {
    await app.subjects.create(op(), {
      id: subjectId,
      displayName: '认知场景主体',
      capabilities: ['Memory'],
    });
    const subject = await delegate(
      { kind: 'subject', host: 'nous', id: subjectId },
      [
        'subjects.read',
        'settings.read',
        'materials.capture',
        'materials.read',
        'encounters.read',
        'encounters.accept',
        'contexts.prepare',
        'knowledge.read',
      ],
    );
    ctx = subject();
    const modelResourceId = `growth-${suffix}`;
    const credential = await app.secrets.create(op(), {
      purpose: 'model.provider',
      resourceRef: {
        product: 'nous',
        installationId: app.installationId,
        kind: 'Provider',
        id: 'fixture',
      },
      allowedConsumers: [
        administrative.consumer,
        { kind: 'worker', host: app.installationId, id: '*' },
      ],
      backend: {
        kind: 'protected_file',
        material: Buffer.from('local-fixture'),
      },
    });
    const installation = {
      kind: 'installation' as const,
      installationId: app.installationId,
    };
    await app.models.providers.create(op(), {
      expectedRevision: 0,
      definition: {
        id: 'fixture',
        adapter: 'openai-compatible',
        baseURL,
        credentialRef: credential.ref,
        networkProfileId: 'default',
        options: {
          supportsStructuredOutputs: true,
          addressScope: 'configured_private',
          allowedPrivateRanges: ['127.0.0.0/8'],
        },
        enabled: true,
      },
    });
    await app.models.resources.create(op(), {
      expectedRevision: 0,
      definition: {
        managedBy: 'sdk',
        definition: {
          id: modelResourceId,
          label: '认知真实SDK场景',
          providerId: 'fixture',
          modelId: 'growth-json',
          kind: 'language',
          enabled: true,
          capabilities: ['text', 'structured'],
          contextLimit: 65536,
          outputLimit: 4096,
          providerOptions: {},
          pricingRef: 'fixture',
        },
      },
    });
    await app.settings.update(op(), {
      namespace: 'platform.modelPrices',
      scope: installation,
      expectedRevision: 0,
      patch: [
        {
          op: 'add',
          path: '/prices',
          value: {
            fixture: {
              id: 'fixture',
              rates: [
                {
                  unit: 'input_tokens',
                  amount: { amount: '1', currency: 'USD' },
                  per: '1000000',
                },
                {
                  unit: 'output_tokens',
                  amount: { amount: '1', currency: 'USD' },
                  per: '1000000',
                },
              ],
            },
          },
        },
      ],
    });
    await app.settings.update(op(), {
      namespace: 'platform.roles',
      scope: installation,
      expectedRevision: 0,
      patch: [
        {
          op: 'add',
          path: '/roles',
          value: Object.fromEntries(
            roleCatalog
              .filter((role) => role.kind === 'language')
              .map((role) => [
                role.id,
                {
                  ...role.defaultPolicy,
                  enabled: true,
                  resources: [{ id: modelResourceId, revision: 1 }],
                  inference: { maxOutputTokens: 4096, providerOptions: {} },
                },
              ]),
          ),
        },
      ],
    });
    const material = await app.materials.capture(ctx, {
      kind: 'message',
      mediaType: 'text/plain',
      text: '小林：我们周六下午去青柠咖啡馆吧。',
      source: { roots: [] },
    });
    await app.host.start();
    const view = await app.contexts.prepare(ctx, {
      consumer: ctx.consumer,
      role: 'deliberation',
      roleTask: '阅读当前收到的消息。',
      latestMaterials: [
        { ref: material.ref, text: material.text!, required: true },
      ],
      model: {
        resourceId: 'local-fixture',
        modelId: 'subject-reader',
        contextLimit: 4096,
      },
      budget: { outputTokens: 256 },
      observed,
    });
    assert.equal(
      (await app.encounters.list(ctx)).length,
      0,
      'preparation has no encounter side effect',
    );
    const actual = await fetch(`${baseURL}/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        model: 'subject-reader',
        messages: [
          {
            role: 'user',
            content: view.blocks.map((block) => block.text).join('\n'),
          },
        ],
      }),
    });
    assert.equal(actual.status, 200);
    const output = (await actual.json()) as {
      choices: Array<{ message: { content: string } }>;
    };
    const result = await app.materials.capture(ctx, {
      kind: 'model-output',
      mediaType: 'text/plain',
      text: output.choices[0]!.message.content,
      source: { inputs: [material.ref], roots: [] },
    });
    const accept = {
      consumer: view.consumer,
      invocationId: `reader-${suffix}`,
      step: 0,
      contextViewRef: view.ref,
      portions: view.inputPortions.map((portion) => ({
        ref: { ...portion.ref, selector: portion.selector },
      })),
      resultRef: result.ref,
      observed,
      purpose: ctx.purpose,
    };
    const first = await app.encounters.accept(ctx, accept);
    assert.equal(
      (await app.encounters.accept(ctx, accept)).ref.id,
      first.ref.id,
    );
    assert.equal((await app.encounters.experiences(ctx)).items.length, 1);
    const growth = await app.growth.run(op(), {
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:01:00.000000Z',
      },
    });
    assert.equal(growth.createdRefs.length, 1);
    assert.ok(growth.artifact?.modelOperationId);
    const formationCalls=await app.models.invocations.list(op(),{logicalOperationId:growth.artifact.modelOperationId});
    assert.equal(formationCalls.items.length,1);assert.equal(formationCalls.items[0]!.traceId.length,32);

    const beforeInstallation = (await app.encounters.experiences(ctx)).items
      .length;
    const installationCtx = makeContext({
      ...administrative,
      purpose: 'conversation',
      audience: [subjectId],
    });
    const tested = await app.models.text.generate(installationCtx, {
      roleId: 'subject_reflection',
      inputRefs: [],
      promptParams: {},
      options: { prompt: '安装级连通测试。', maxOutputTokens: 128 },
    });
    assert.match(tested.text, /groups/);
    const installationCalls = (
      await app.models.invocations.list(installationCtx)
    ).items.filter((call) => call.subjectId === undefined);
    assert.equal(installationCalls.length, 1);
    assert.equal(installationCalls[0]!.outputRefs[0]!.kind, 'ModelArtifact');
    assert.equal(
      (await app.encounters.experiences(ctx)).items.length,
      beforeInstallation,
      '安装级实际测试没有伪造主体经历',
    );

    const recalled = await app.knowledge.query(ctx, '青柠咖啡馆');
    assert.match(recalled.items[0]?.excerpt ?? '', /周六下午/);
    const beforeEmpty = providerCalls;
    await app.growth.run(op(), {});
    assert.equal(
      providerCalls,
      beforeEmpty,
      'no new experiences cause no model call',
    );
    const costs = (await app.models.costs.read(op())).items;
    assert.ok(
      costs.some(
        (row) =>
          row.roleId === 'subject_reflection' &&
          row.status === 'completed' &&
          row.actualCost !== null,
      ),
    );
    const formed = {
      domainId: observed.domainId,
      at: '2026-10-20T08:02:00.000000Z',
    };
    await app.subjects.provision(op(), [
      'Memory',
      'Episode',
      'Journal',
      'Concept',
      'Motivation',
    ]);
    const experience = (await app.encounters.experiences(ctx)).items[0]!;
    const organized = await app.episodes.organize(op(), {
      track: 'social',
      experienceRefs: [experience.ref],
      formed,
      reason: '一次完整的咖啡馆约定',
      episodes: [
        {
          title: '和小林约好周末见面',
          summary: '读到了小林邀请周六下午去青柠咖啡馆的消息。',
          range: { start: observed, end: formed },
          memberRefs: [experience.ref],
        },
      ],
    });
    assert.equal(organized.items.length, 1);
    assert.equal(
      (await app.episodes.forExperience(ctx, experience.ref)).length,
      1,
    );
    const journal = await app.journals.write(op(), {
      content: '今天和小林约好了周末去青柠咖啡馆，期待见面。',
      mood: '期待',
      range: { start: observed, end: formed },
      experienceRefs: [experience.ref],
      knowledgeAt: formed,
      formed,
    });
    assert.equal(journal.body.kind, 'Journal');
    const concept = (name: string) =>
      app.knowledge.create(op(), {
        kind: 'Concept',
        body: { kind: 'Concept', content: name, name },
        sourceRefs: [material.ref],
        formed,
        formation: 'inferred',
        stance: 'accepted',
      });
    const primary = await concept('咖啡馆');
    const alias = await concept('青柠小店');
    const learned = await app.knowledge.read(ctx, {
      id: growth.createdRefs[0]!.id,
    });
    await app.knowledge.revise(op(), {
      kind: learned.kind,
      body: { ...learned.body, conceptRefs: [alias.ref] },
      sourceRefs: learned.sourceRefs,
      formed,
      formation: 'reflection',
      stance: learned.stance,
      id: learned.ref.id,
      expectedRevision: learned.revision,
      reason: '以小店主题组织这次约定',
    });
    const merged = await app.concepts.merge(op(), {
      kept: { id: primary.ref.id, expectedRevision: primary.revision },
      merged: [{ id: alias.ref.id, expectedRevision: alias.revision }],
      formed,
      reason: '小店是同一家咖啡馆',
    });
    const tagged = await app.knowledge.read(ctx, { id: learned.ref.id });
    assert.equal(tagged.body.conceptRefs[0]?.id, primary.ref.id);
    const beforeSplit = await app.knowledge.list(ctx, {
      includeHistory: true,
      limit: 200,
    });
    assert.equal(beforeSplit.nextCursor, undefined);
    await assert.rejects(
      app.concepts.split(op(), {
        source: {
          id: merged.kept.alias,
          expectedRevision: merged.kept.revision,
        },
        formed,
        reason: '检查完整概念采用组',
        branches: [
          {
            name: '回滚地点',
            definition: '按地点区分',
            memberRefs: [tagged.ref],
          },
          {
            name: '回滚活动',
            definition: '按活动区分',
            memberRefs: [journal.ref],
          },
        ],
      }),
      /未引用原概念/,
    );
    assert.deepEqual(
      (
        await app.knowledge.list(ctx, { includeHistory: true, limit: 200 })
      ).items.map((item) => [item.ref.id, item.revision]),
      beforeSplit.items.map((item) => [item.ref.id, item.revision]),
      'failed grouping rolls back branches and a previously revised member',
    );
    assert.equal(
      (await app.knowledge.read(ctx, { id: tagged.ref.id })).revision,
      tagged.revision,
    );
    const split = await app.concepts.split(op(), {
      source: {
        id: merged.kept.ref.id,
        expectedRevision: merged.kept.revision,
      },
      formed,
      reason: '区分地点与交往活动',
      branches: [
        {
          name: '青柠咖啡馆地点',
          definition: '青年公园旁的青柠咖啡馆。',
          memberRefs: [tagged.ref],
        },
        {
          name: '咖啡馆交往',
          definition: '和朋友去咖啡馆交谈的活动。',
          memberRefs: [],
        },
      ],
    });
    assert.equal(
      (await app.knowledge.read(ctx, { id: learned.ref.id })).body
        .conceptRefs[0]?.id,
      split.branches[0]?.ref.id,
    );
    const fact = await app.materials.capture(ctx, {
      kind: 'world-event',
      mediaType: 'application/json',
      text: JSON.stringify({ practice: { completed: true } }),
      occurred: { start: formed },
      source: { roots: [] },
    });
    const expectation = await app.knowledge.create(op(), {
      kind: 'Motivation',
      body: {
        kind: 'Motivation',
        content: '在九点前完成一次练习。',
        aspect: 'evaluable_expectation',
        importance: 0.8,
        criterion: {
          mode: 'achieve',
          range: {
            start: observed,
            end: {
              domainId: observed.domainId,
              at: '2026-10-20T09:00:00.000000Z',
            },
          },
          condition: '练习已完成',
          predicate: {
            path: ['practice', 'completed'],
            operator: 'eq',
            value: true,
          },
        },
      },
      sourceRefs: [fact.ref],
      formed,
      formation: 'operator_edit',
      stance: 'accepted',
    });
    const evaluated = await app.motivation.evaluate(op(), {
      id: expectation.ref.id,
      observationRefs: [fact.ref],
      coverage: {
        range: { start: observed, end: formed },
        ratio: 0.1,
        sourceRefs: [fact.ref],
      },
      evaluatedAt: {
        domainId: observed.domainId,
        at: '2026-10-20T08:03:00.000000Z',
      },
    });
    assert.equal(evaluated.verdict, 'satisfied');
    assert.equal(evaluated.criterionRevision, expectation.revision);
    assert.equal(
      (await app.motivation.history(ctx, expectation.ref.id)).length,
      1,
    );
    const image = Buffer.from(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j2ioAAAAASUVORK5CYII=',
      'base64',
    );
    const upload = op();
    const blob = await app.materials.putBlobStream(
      upload,
      Readable.from([image.subarray(0, 16), image.subarray(16)]),
      'image/png',
      128,
    );
    assert.equal(blob.bytes, image.length);
    await assert.rejects(app.materials.readBlob(ctx, blob.ref), /材料持有者/);
    assert.equal(
      (
        await app.materials.putBlobStream(
          upload,
          Readable.from([image]),
          'image/png',
          128,
        )
      ).ref.id,
      blob.ref.id,
    );
    await assert.rejects(
      app.materials.putBlobStream(
        upload,
        Readable.from([Buffer.from('different')]),
        'image/png',
        128,
      ),
      /提交.*输入|不同的原件/,
    );
    await assert.rejects(
      app.materials.putBlobStream(op(), Readable.from([image]), 'image/png', 2),
      /单件上传大小/,
    );
    const privateImage = await app.materials.capture(op(), {
      kind: 'image',
      mediaType: 'image/png',
      blobRef: blob.ref,
      source: { inputs: [material.ref] },
    });
    await assert.rejects(app.materials.readBlob(ctx, blob.ref), /材料持有者/);
    const imageMaterial = await app.materials.publish(op(), {
      ref: privateImage.ref,
    });
    assert.deepEqual(
      (await app.materials.readBlob(ctx, blob.ref)).bytes,
      image,
    );
    const currentMemory = await app.knowledge.read(ctx, { id: learned.ref.id });
    const summaryDraft = await app.materials.capture(op(), {
      kind: 'summary',
      mediaType: 'text/plain',
      text: '小林和我约好了咖啡馆。',
      source: { inputs: [{ ...currentMemory.ref, revision: undefined }] },
    });
    assert.equal(
      summaryDraft.source.inputs[0]?.revision,
      currentMemory.revision,
      'direct knowledge dependency pins the actual read revision',
    );
    assert.equal(
      summaryDraft.source.roots.some((ref) => ref.kind === 'Memory'),
      false,
      'a memory representation does not become an independent source root',
    );
    const summary = await app.materials.publish(op(), {
      ref: summaryDraft.ref,
    });
    const native = await delegate(
      { kind: 'worker', host: 'openclaw-fixture', id: 'retained-run' },
      [
        'subjects.read',
        'settings.read',
        'materials.read',
        'knowledge.read',
        'contexts.prepare',
        'contexts.read',
      ],
    );
    const nativeHistory = {
      consumer: {
        kind: 'worker' as const,
        host: 'openclaw-fixture',
        id: 'retained-run',
      },
      role: 'worker',
      requiredSources: [summary.ref],
      model: {
        resourceId: 'local-fixture',
        modelId: 'subject-reader',
        contextLimit: 4096,
      },
      budget: { outputTokens: 256 },
      observed,
    };
    const inherited = await app.contexts.prepare(native(), nativeHistory);
    assert.equal(
      inherited.blocks.length,
      0,
      'retained native history adds source dependencies without copying its text',
    );
    assert.equal(inherited.inputPortions[0]?.text, '');
    const workerCount = (await app.encounters.list(op())).filter(
      (row) => row.body.consumer.kind !== 'subject',
    ).length;
    const completedCosts = (await app.models.costs.read(op())).items.filter(
      (row) => row.status === 'completed',
    ).length;
    invalidStructured = true;
    await assert.rejects(
      app.seeds.preview(op(), { refs: [fact.ref] }),
      (error) => {
        assert.ok(error instanceof Error);
        assert.equal(
          (error as Error & { problem: { code: string } }).problem.code,
          'models.output_invalid',
        );
        assert.equal(
          error.message.includes('这次实际模型输出不是 JSON'),
          false,
        );
        return true;
      },
    );
    invalidStructured = false;
    const workers = (await app.encounters.list(op())).filter(
      (row) => row.body.consumer.kind !== 'subject',
    );
    assert.equal(
      workers.length,
      workerCount + 1,
      'successful raw output is an actual worker encounter before SDK schema validation',
    );
    assert.match(
      (await app.materials.read(op(), { ref: workers.at(-1)!.body.resultRef! }))
        .text ?? '',
      /这次实际模型输出不是 JSON/,
    );
    assert.equal(
      (await app.models.costs.read(op())).items.filter(
        (row) => row.status === 'completed',
      ).length,
      completedCosts + 1,
      'completed call is settled despite an invalid structured result',
    );
    assert.equal(
      (await app.encounters.experiences(ctx)).items.length,
      1,
      'worker private processing does not add a subject experience',
    );
    const worker = await delegate(
      { kind: 'worker', host: 'scenario', id: 'job-1' },
      ['materials.capture', 'materials.read', 'materials.publish'],
    );
    const draft = await app.materials.capture(worker(), {
      kind: 'worker-draft',
      mediaType: 'text/plain',
      text: '检查完成。私有草稿推理。',
      source: { inputs: [material.ref] },
    });
    await assert.rejects(
      app.materials.read(subject(), { ref: draft.ref }),
      isAccessDenied,
    );
    await assert.rejects(
      app.knowledge.sources.resolve(subject(), [draft.ref]),
      isAccessDenied,
    );
    const published = await app.materials.publish(worker(), {
      ref: { ...draft.ref, selector: { kind: 'text', start: 0, end: 5 } },
      title: '检查成果',
    });
    assert.equal(
      (await app.materials.read(subject(), { ref: published.ref })).text,
      '检查完成。',
    );
    const nodeDraft = await app.materials.capture(worker(), {
      kind: 'native-output',
      mediaType: 'application/json',
      text: JSON.stringify({
        content: [
          { type: 'text', text: '选定成果。' },
          { type: 'thinking', text: '未发布的私有分析。' },
        ],
      }),
      source: { inputs: [material.ref] },
    });
    const nodePublished = await app.materials.publish(worker(), {
      ref: {
        ...nodeDraft.ref,
        selector: { kind: 'node', pointer: '/content/0/text' },
      },
    });
    assert.equal(
      (await app.materials.read(subject(), { ref: nodePublished.ref })).text,
      '选定成果。',
    );
    assert.equal(
      (await app.encounters.experiences(ctx)).items.length,
      1,
      'publishing a worker result alone creates no subject experience',
    );
    await app.encounters.accept(subject(), {
      consumer: { kind: 'subject', host: 'nous', id: subjectId },
      invocationId: `published-${suffix}`,
      step: 0,
      portions: [{ ref: published.ref }],
      observed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:06:00.000000Z',
      },
      purpose: 'conversation',
    });
    const target = await app.knowledge.read(ctx, { id: learned.ref.id });
    const proposal = (
      id: string,
      content: string,
      targetRevision?: number,
    ) => ({
      id,
      reason: '根据实际查看的成果整理',
      changes: [
        {
          reason: '对应当前认识',
          kind: 'Memory',
          body: { kind: 'Memory', content },
          sourceIds: ['s1'],
          formed,
          formation: 'reflection',
          stance: 'accepted',
          ...(targetRevision
            ? {
                target: { id: target.ref.id, expectedRevision: targetRevision },
              }
            : {}),
        },
      ],
    });
    proposedGroups = [
      proposal('corrected', '成果对应修订', target.revision),
      proposal('independent', '已经查看了检查成果。'),
    ];
    const pending = await app.growth.run(op(), {
      apply: false,
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:07:00.000000Z',
      },
    });
    assert.ok(pending.artifact);
    const revised = await app.knowledge.revise(op(), {
      ...target,
      body: { ...target.body, content: '管理员补充了一处说明。' },
      id: target.ref.id,
      expectedRevision: target.revision,
      formation: 'operator_edit',
      reason: '与后台成长并发的明确修订',
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:07:00.000000Z',
      },
    });
    const conflicted = await app.growth.apply(op(), {
      id: pending.artifact.id,
    });
    assert.equal(
      conflicted.artifact.groups.find((group) => group.id === 'corrected')
        ?.status,
      'conflict',
    );
    const independent = conflicted.artifact.groups.find(
      (group) => group.id === 'independent',
    )!.createdRefs[0]!;
    await app.encounters.accept(subject(), {
      consumer: { kind: 'subject', host: 'nous', id: subjectId },
      invocationId: `practice-${suffix}`,
      step: 0,
      portions: [{ ref: fact.ref }],
      observed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:08:00.000000Z',
      },
      purpose: 'conversation',
    });
    proposedGroups = [proposal('next-batch', '实际查看了练习完成情况。')];
    const later = await app.growth.run(op(), {
      apply: false,
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:09:00.000000Z',
      },
    });
    assert.ok(later.artifact);
    assert.notEqual(
      later.artifact.id,
      pending.artifact.id,
      'an unresolved prior group does not hold later experiences',
    );
    const nextChange = later.artifact.changeSet.groups[0]!.changes[0]!;
    const invalid = await app.growth.resolve(op(), {
      id: later.artifact.id,
      groupIds: ['next-batch'],
      action: 'edit',
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:10:00.000000Z',
      },
      reason: '检查未取得来源的处理',
      groups: [
        {
          id: 'next-batch',
          reason: '待确认来源',
          changes: [
            {
              ...nextChange,
              sourceRefs: [
                { ...material.ref, id: '019a05b0-0000-7000-8000-00000000dead' },
              ],
            },
          ],
        },
      ],
    });
    assert.equal(invalid.artifact.groups[0]?.status, 'invalid');
    const discarded = await app.growth.resolve(op(), {
      id: later.artifact.id,
      groupIds: ['next-batch'],
      action: 'discard',
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:10:00.000000Z',
      },
      reason: '结束这项候选',
    });
    assert.equal(discarded.artifact.groups[0]?.status, 'discarded');
    proposedGroups = [
      proposal('corrected', '仅重新判断发生冲突的修订。', revised.revision),
    ];
    const callsBeforeResolve = providerCalls;
    const resolved = await app.growth.resolve(op(), {
      id: pending.artifact.id,
      groupIds: ['corrected'],
      action: 'rejudge',
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:11:00.000000Z',
      },
      reason: '以当前修订重判冲突组',
    });
    assert.equal(providerCalls, callsBeforeResolve + 1);
    assert.equal(resolved.pendingGroups.length, 0);
    assert.equal(
      resolved.artifact.groups.find((group) => group.id === 'independent')!
        .createdRefs[0]!.id,
      independent.id,
      'independent adoption is not repeated',
    );
    assert.equal(resolved.artifact.resolutions[0]?.previous.groups.length, 1);
    proposedGroups = undefined;
    const seed = await app.seeds.preview(op(), { refs: [fact.ref] });
    const selectedAt = {
      domainId: observed.domainId,
      at: '2026-10-20T08:12:00.000000Z',
    };
    const seeded = await Promise.all([
      app.seeds.apply(op(), { id: seed.artifact.id, formed: selectedAt }),
      app.seeds.apply(op(), { id: seed.artifact.id, formed: selectedAt }),
    ]);
    assert.equal(
      seeded[0]!.createdRefs[0]?.id,
      seeded[1]!.createdRefs[0]?.id,
      'concurrent seed adoption creates one formal result',
    );
    assert.equal(
      (await app.knowledge.read(ctx, { id: seeded[0]!.createdRefs[0]!.id }))
        .formed.domainId,
      observed.domainId,
    );
    const firstPage = await app.knowledge.list(ctx, { limit: 2 });
    assert.equal(firstPage.items.length, 2);
    assert.ok(firstPage.nextCursor);
    const secondPage = await app.knowledge.list(ctx, {
      limit: 2,
      cursor: firstPage.nextCursor,
    });
    assert.equal(
      secondPage.items.some((item) =>
        firstPage.items.some((first) => first.ref.id === item.ref.id),
      ),
      false,
    );
    await assert.rejects(
      app.knowledge.list(ctx, {
        kinds: ['Memory'],
        limit: 2,
        cursor: firstPage.nextCursor,
      }),
      /游标.*查询/,
    );
    const historical = await app.knowledge.read(ctx, {
      id: learned.alias,
      knowledgeAt: {
        domainId: observed.domainId,
        at: '2026-10-20T08:01:30.000000Z',
      },
    });
    assert.equal(
      historical.revision,
      1,
      'historical selection precedes later concept organization',
    );
    const memoryHistory = await app.knowledge.history(ctx, learned.alias, {
      limit: 1,
    });
    assert.equal(memoryHistory.items[0]?.revision, 1);
    assert.ok(memoryHistory.nextCursor);
    assert.equal(
      (
        await app.knowledge.history(ctx, learned.alias, {
          limit: 1,
          cursor: memoryHistory.nextCursor,
        })
      ).items[0]?.revision,
      2,
    );
    const forbiddenFuture = await app.materials.capture(op(), {
      kind: 'message',
      mediaType: 'text/plain',
      text: '下午才会读到的纠正。',
      source: { roots: [] },
    });
    await assert.rejects(
      app.knowledge.create(op(), {
        kind: 'Memory',
        body: { kind: 'Memory', content: '上午已理解了下午的纠正' },
        sourceRefs: [forbiddenFuture.ref],
        formed,
        formation: 'reflection',
        stance: 'accepted',
      }),
      /早于实际取得/,
    );
    const relationOne = await app.knowledge.relate(op(), {
      fromId: learned.alias,
      toId: split.branches[0]!.alias,
      relation: 'about_place',
      evidenceRefs: [experience.ref],
      formed,
    });
    await app.knowledge.relate(op(), {
      fromId: learned.ref.id,
      toId: split.branches[0]!.ref.id,
      relation: 'about_place',
      weight: 0.5,
      evidenceRefs: [experience.ref],
      formed: {
        domainId: observed.domainId,
        at: '2026-10-20T08:05:00.000000Z',
      },
    });
    const relationPast = await app.knowledge.relations(ctx, [learned.alias], {
      knowledgeAt: {
        domainId: observed.domainId,
        at: '2026-10-20T08:04:00.000000Z',
      },
    });
    assert.equal(
      relationPast.items.find(
        (item) => item.relationId === relationOne.relationId,
      )?.revision,
      1,
      'relation weights retain their subject-clock history',
    );
    const actualMemory = await app.knowledge.read(ctx, { id: learned.ref.id });
    await app.knowledge.setStatus(op(), {
      id: actualMemory.alias,
      expectedRevision: actualMemory.revision,
      status: 'suppressed',
    });
    assert.equal(
      (await app.knowledge.read(ctx, { id: actualMemory.alias })).status,
      'suppressed',
    );
    await app.knowledge.setStatus(op(), {
      id: actualMemory.alias,
      expectedRevision: actualMemory.revision,
      status: 'active',
    });
    await app.materials.updatePolicy(op(), {
      ref: material.ref,
      policy: {
        readers: [subjectId],
        purposes: ['private-review'],
        audiences: [subjectId],
      },
    });
    const restricted = await app.knowledge.query(ctx, '青柠咖啡馆');
    assert.equal(
      restricted.items.some((item) => item.ref.id === learned.ref.id),
      false,
      'source policy excludes the derived memory before index cleanup',
    );
    await assert.rejects(
      app.knowledge.read(ctx, { id: learned.ref.id }),
      isAccessDenied,
    );
    await assert.rejects(
      app.materials.read(subject(), { ref: published.ref }),
      isAccessDenied,
    );
    await assert.rejects(
      app.materials.read(subject(), { ref: nodePublished.ref }),
      isAccessDenied,
    );
    await assert.rejects(
      app.materials.read(ctx, { ref: summary.ref }),
      isAccessDenied,
    );
    await assert.rejects(
      app.materials.read(ctx, { ref: result.ref }),
      isAccessDenied,
    );
    await assert.rejects(
      app.materials.read(ctx, { ref: imageMaterial.ref }),
      isAccessDenied,
    );
    await assert.rejects(app.materials.readBlob(ctx, blob.ref), /材料持有者/);
    await assert.rejects(
      app.materials.capture(op(), {
        kind: 'image',
        mediaType: 'image/png',
        blobRef: blob.ref,
        source: { roots: [] },
      }),
      /材料持有者/,
    );
    await assert.rejects(
      app.materials.read(op(), { ref: workers.at(-1)!.body.resultRef! }),
      isAccessDenied,
    );
    await assert.rejects(
      app.contexts.prepare(native(), nativeHistory),
      isAccessDenied,
    );
    await assert.rejects(
      app.contexts.readView(op(), { id: inherited.id }),
      isAccessDenied,
    );
  } finally {
    await app.host.stop();
    await new Promise<void>((resolve, reject) =>
      provider.close((error) => (error ? reject(error) : resolve())),
    );
    await rm(directory, { recursive: true, force: true });
  }
});
