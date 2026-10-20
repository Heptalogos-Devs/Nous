import assert from 'node:assert/strict';
import test from 'node:test';
import { resolveQuery, reciprocalRankFusion, residualVector } from '../packages/nous/src/retrieval/index.js';
import { sameRef, TimePointSchema, type Ref } from '@nous-wave/contracts/common';
import { criterionVerdict, evaluatePredicate } from '../packages/nous/src/knowledge/organization.js';

test('NousQL 与自然语言使用同一个查询结构', () => {
  const cases = [
    { input: '上次和小林讨论的咖啡店', expect: { intent: '上次和小林讨论的咖啡店', limit: 12 } },
    { input: 'find "那次争论" return [Memory, Episode] entity all ["person:alice"] tag ["考试"] limit 8', expect: { intent: '那次争论', kinds: ['Memory', 'Episode'], entities: ['person:alice'], entityMatch: 'all', tags: ['考试'], limit: 8 } },
    { input: 'FIND "当时知道什么" TIME observed IN "world:campus" BETWEEN "2026-10-01T00:00:00Z" AND "2026-10-20T00:00:00Z" KNOWLEDGE_AT "world:campus" "2026-10-12T08:00:00Z" RECORDED_AS_OF "2026-10-20T01:00:00Z" PREFER [recent, independent_sources] EXPLORE 2 EXPAND [sources, relations]', expect: { knowledgeAt: { clockId: 'world:campus', at: '2026-10-12T08:00:00.000Z' }, recordedAsOf: '2026-10-20T01:00:00.000Z', exploration: 2, preferences: ['recent', 'independent_sources'], expand: ['sources', 'relations'] } },
  ];
  for (const { input, expect } of cases) {
    const actual = resolveQuery(input);
    for (const [key, value] of Object.entries(expect)) assert.deepEqual(actual[key as keyof typeof actual], value, input);
  }
});

test('NousQL 定位语法、重复指令、范围和正整数错误', () => {
  for (const input of [
    'FIND "问题" LIMIT 0',
    'FIND "问题" LIMIT 2 LIMIT 3',
    'FIND "问题" RETURN [Unknown]',
    'FIND "问题" ENTITY ["未闭合]',
    'FIND "问题" TIME formed IN "real" BETWEEN "2026-10-20T00:00:00Z" AND "2026-10-01T00:00:00Z"',
    'FIND "问题" TAG ["有效"] 错误尾部',
  ]) assert.throws(() => resolveQuery(input), /NousQL/, input);
  assert.equal(resolveQuery('FINDings 是一个普通词').intent, 'FINDings 是一个普通词');
});

test('候选融合按对象去重；残差只保留未覆盖方向', () => {
  const scores = reciprocalRankFusion([['a', 'b'], ['b', 'c']]);
  assert.equal(scores.size, 3);
  assert.ok(scores.get('b')! > scores.get('a')!);
  const residual = residualVector([1, 1, 0], [[1, 0, 0]]);
  assert.ok(residual);
  assert.ok(Math.abs(residual[0]!) < 1e-9);
  assert.ok(Math.abs(residual[1]! - 1) < 1e-9);
  assert.equal(residualVector([1, 0], [[1, 0]]), undefined);
});

test('来源区域身份与 PostgreSQL JSONB 对象键顺序无关', () => {
  const source: Ref = { product: 'nous', installationId: 'test', subjectId: 'subject', kind: 'Material', id: 'material', revision: 1, selector: { kind: 'text', start: 0, end: 18, within: { kind: 'page', page: 2 } } };
  const persisted = { selector: { within: { page: 2, kind: 'page' }, end: 18, start: 0, kind: 'text' }, revision: 1, id: 'material', kind: 'Material', subjectId: 'subject', installationId: 'test', product: 'nous' } as Ref;
  assert.ok(sameRef(source, persisted));
  assert.equal(sameRef(source, { ...source, selector: { kind: 'text', start: 0, end: 19, within: source.selector?.within } }), false);
  assert.equal(TimePointSchema.parse({ clockId: 'real', at: '2026-10-20T09:00:00+09:00' }).at, '2026-10-20T00:00:00.000Z');
});

test('四类期望按正反事实、期限与观察覆盖评价', () => {
  const cases: Array<[Parameters<typeof criterionVerdict>[0], Array<boolean | undefined>, boolean, boolean, string]> = [
    ['achieve', [true], false, false, 'satisfied'], ['achieve', [], true, true, 'violated'], ['achieve', [], false, true, 'unknown'],
    ['terminate', [false], false, false, 'satisfied'], ['terminate', [true], true, true, 'violated'], ['terminate', [], true, true, 'unknown'],
    ['maintain', [false], false, false, 'violated'], ['maintain', [true], true, true, 'satisfied'], ['maintain', [], true, true, 'unknown'],
    ['avoid', [true], false, false, 'violated'], ['avoid', [], true, true, 'satisfied'], ['avoid', [false], true, false, 'unknown'],
  ];
  for (const [mode, values, expired, covered, expected] of cases) assert.equal(criterionVerdict(mode, values, expired, covered), expected, mode);
  for (const mode of ['achieve', 'terminate', 'maintain', 'avoid'] as const) assert.equal(criterionVerdict(mode, [undefined], true, true), 'unknown');
  assert.equal(evaluatePredicate({ study: { completed: 3 } }, { path: ['study', 'completed'], operator: 'gte', value: 3 }), true);
  assert.equal(evaluatePredicate({}, { path: ['study', 'completed'], operator: 'gte', value: 3 }), undefined);
  assert.throws(() => evaluatePredicate({}, { path: ['__proto__'], operator: 'exists' }), /属性路径/);
});
