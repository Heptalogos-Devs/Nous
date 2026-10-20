import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import type { CallContext } from '@nous-wave/contracts/common';
import { KnowledgeViewSchema, type KnowledgeView } from '@nous-wave/contracts/knowledge';
import { createRetrieval } from '../packages/nous/src/retrieval/index.js';
import { authorize, mayRead } from '../packages/nous/src/platform/security.js';

test('连续中文经历经原生 ICU 索引、历史修订和权限回读进入第二次召回', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'nous-recall-'));
  const subjectId = '019a05b0-0000-7000-8000-000000000001';
  const ctx: CallContext = { subjectId, actor: { kind: 'human', id: 'operator', grants: ['*'], subjectIds: [subjectId] }, purpose: 'conversation', audience: [subjectId], traceContext: { operationId: 'recall-scenario', traceId: '0'.repeat(32) } };
  const make = (id: string, revision: number, content: string, formed: string, root: string) => KnowledgeViewSchema.parse({
    ref: { product: 'nous', installationId: 'scenario', subjectId, kind: 'Memory', id, revision },
    kind: 'Memory', body: { kind: 'Memory', content }, alias: `ref-${id}`, revision,
    sourceRefs: [{ product: 'nous', installationId: 'scenario', subjectId, kind: 'Material', id: root, revision: 1 }],
    sourceRoots: [{ product: 'hept', installationId: 'local', subjectId, kind: 'Message', id: root }],
    formed: { clockId: 'world:campus', at: formed }, recorded: { clockId: 'real', at: formed },
    formation: 'reflection', stance: 'accepted', confidence: 0.8, policy: { readers: [subjectId], purposes: ['conversation'], audiences: [subjectId] }, status: 'active',
  });
  const monday = make('019a05b0-0000-7000-8000-000000000011', 1, '星期一和小林在青年公园旁的青柠咖啡馆讨论连续时间模型，并且约好周六下午再去看看窗边新添的植物。', '2026-10-12T08:00:00.000Z', 'monday-chat');
  const wednesday = make(monday.ref.id, 2, '星期三小林纠正了约定：周六要去青年公园旁的青柠咖啡馆，时间改为上午十点。', '2026-10-14T08:00:00.000Z', 'wednesday-chat');
  const other = make('019a05b0-0000-7000-8000-000000000012', 1, '今天整理了编译器与类型系统的学习笔记。', '2026-10-12T07:00:00.000Z', 'study');
  let restricted = false;
  const select = (knowledgeAt?: { clockId: string; at: string }) => [knowledgeAt && knowledgeAt.at < wednesday.formed.at ? monday : wednesday, other].filter(view => !restricted && mayRead(ctx, view.policy));
  let embeddingCalls = 0;
  const retrieval = createRetrieval({
    installationId: 'scenario', indexPath: directory, authorize,
    knowledge: {
      async list(_ctx, input) { return {items:input.includeHistory?[monday,wednesday,other]:select(input.knowledgeAt)}; },
      async readMany(_ctx,inputs){return select(inputs[0]?.knowledgeAt).filter(view=>inputs.some(input=>input.id===view.ref.id));},
      async read(_ctx, input): Promise<KnowledgeView> { const item = [monday, wednesday, other].find(view => view.ref.id === input.id && (!input.revision || view.revision === input.revision)); if (!item || restricted) throw new Error('not readable'); return item; },
    },
    embed: { id: 'scenario-controlled-space', dimensions: 3, async embed(_ctx, texts) { embeddingCalls += texts.length; return texts.map(text => [1 + Number(text.includes('咖啡')), 1 + Number(text.includes('编译')), 1 + Number(text.includes('小林'))]); } },
  });
  try {
    const first = await retrieval.query(ctx, '青柠咖啡馆');
    assert.equal(first.items[0]?.ref.revision, 2);
    assert.match(first.items[0]?.excerpt ?? '', /上午十点/);
    assert.equal(first.indexState.tokenizer, 'icu');
    assert.equal(first.indexState.vectorReady, true);
    const afterIndex = embeddingCalls;
    const again = await retrieval.query(ctx, '青柠咖啡馆');
    assert.equal(embeddingCalls, afterIndex + 1, 'immutable revisions reuse their stored vectors');
    assert.equal((await retrieval.readResult(ctx, again.resultSetId, 1)).revision, 2);
    const historical = await retrieval.query(ctx, 'FIND "青柠咖啡馆" KNOWLEDGE_AT "world:campus" "2026-10-13T08:00:00Z"');
    assert.equal(historical.items[0]?.ref.revision, 1);
    assert.match(historical.items[0]?.excerpt ?? '', /周六下午/);
    restricted = true;
    assert.equal((await retrieval.query(ctx, '青柠咖啡馆')).items.length, 0, 'revoked authority excludes already indexed content');
  } finally { await retrieval.close(); await rm(directory, { recursive: true, force: true }); }
});
