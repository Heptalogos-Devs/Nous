import assert from 'node:assert/strict';
import test from 'node:test';
import { reciprocalRankFusion, residualVector } from '../packages/nous/src/retrieval/index.js';
import { sameRef,SelectorSchema, type Ref } from '@nous-wave/contracts/common';
import { criterionVerdict, evaluatePredicate } from '../packages/nous/src/knowledge/organization.js';

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
  const node={...source,selector:{kind:'node' as const,pointer:'/content/0/text'}};
  assert.ok(sameRef(node,{...node,selector:{pointer:'/content/0/text',kind:'node'}}));
  assert.equal(SelectorSchema.safeParse({kind:'node',nodeId:'/content/0/text'}).success,false);
  assert.equal(SelectorSchema.safeParse({kind:'node',pointer:'/bad~2escape'}).success,false);
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
