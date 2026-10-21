// Array replacement and immutable snapshots adapted from Nous-Wave configuration.
// Copyright 2026 Aravine Zhu; original mechanisms Apache-2.0.
import deepmerge from '@fastify/deepmerge';
import jsonpatch from 'fast-json-patch';
import { Decimal } from 'decimal.js';
import { problem } from '../errors.js';
import { at, assertChangesWritable, assertJson, assertKnown, assertSparse, digest, immutable, isObject, leaves, parse, parts, within } from './catalog.js';
import type { ConfigScope, ConfigSnapshot, Json, JsonObject, PatchOperation, RevisionRef, SettingsModule } from './types.js';

const merge = deepmerge({ mergeArray: ({ clone }) => (_old, next) => clone(next) });
export interface Layer { ref: RevisionRef; sparse: JsonObject }

function write(document: JsonObject, path: string, value: Json): void {
  const keys = parts(path);
  if (!keys.length) throw problem('settings.invalid_definition', 'validation', '限制字段不能是整个命名空间');
  let current = document;
  for (const key of keys.slice(0, -1)) {
    if (!isObject(current[key])) throw problem('settings.invalid_definition', 'validation', '限制字段路径不是对象');
    current = current[key];
  }
  current[keys.at(-1)!] = structuredClone(value);
}
function numberValue(value: Json): { amount: Decimal; currency?: string } | undefined {
  if (typeof value === 'number') return { amount: new Decimal(value) };
  if (isObject(value) && typeof value.amount === 'string' && /^\d+(?:\.\d+)?$/.test(value.amount) && typeof value.currency === 'string') {
    return { amount: new Decimal(value.amount), currency: value.currency };
  }
  return undefined;
}
export function evaluate(module: SettingsModule, layers: readonly Layer[]): ConfigSnapshot {
  let values = structuredClone(module.defaults) as JsonObject;
  const intermediate: JsonObject[] = [];
  for (const layer of layers) {
    assertSparse(module, layer.sparse);
    values = merge(values, layer.sparse) as JsonObject;
    intermediate.push(values);
  }
  const check = (candidate: JsonObject) => {
    const parsed = parse(module.schema, candidate);
    assertJson(parsed);
    if (!isObject(parsed) || digest(parsed) !== digest(candidate)) throw problem('settings.invalid_value', 'validation', '设置包含未声明值或schema隐式变换');
    return parsed;
  };
  // Bounds cannot hide an invalid explicit value (for example a non-nullable null).
  if (Object.values(module.fields).some(field => field.restriction)) check(values);
  const sources: ConfigSnapshot['sources'] = {};
  for (const path of leaves(values)) {
    let last: Layer | undefined;
    for (const layer of layers) {
      if (leaves(layer.sparse).some(leaf => leaf !== '' && within(path, leaf))) last = layer;
    }
    sources[path] = last ? { source: 'override', revision: structuredClone(last.ref) } : { source: 'default' };
  }
  for (const [path, metadata] of Object.entries(module.fields)) {
    if (!metadata.restriction) continue;
    const explicit = layers.flatMap((layer, index) => {
      const override = at(layer.sparse, path);
      if (!override.found || override.value === null) return [];
      const effective = at(intermediate[index], path);
      return effective.found ? [{ value: effective.value!, layer }] : [];
    });
    if (!explicit.length) continue;
    if (metadata.restriction === 'upper_bound') {
      let smallest = explicit[0];
      let bound = numberValue(smallest.value);
      if (!bound) throw problem('settings.invalid_restriction', 'validation', '上限设置必须是数值或Money', { details: { path } });
      for (const candidate of explicit.slice(1)) {
        const next = numberValue(candidate.value);
        if (!next || next.currency !== bound.currency) throw problem('settings.incompatible_limits', 'validation', '同一设置上限必须使用相同数量类型和币种', { details: { path } });
        if (next.amount.lessThan(bound.amount)) { smallest = candidate; bound = next; }
      }
      write(values, path, smallest.value);
      for (const leaf of leaves(smallest.value, parts(path))) sources[leaf] = { source: 'override', revision: structuredClone(smallest.layer.ref) };
    } else {
      if (explicit.some(item => !Array.isArray(item.value))) throw problem('settings.invalid_restriction', 'validation', '集合限制必须是数组', { details: { path } });
      let intersection = explicit[0].value as Json[];
      for (const candidate of explicit.slice(1)) {
        const allowed = new Set((candidate.value as Json[]).map(digest));
        intersection = intersection.filter(item => allowed.has(digest(item)));
      }
      write(values, path, intersection);
      // Each contributing limit remains in the frozen vector; a set has no single winning layer.
      sources[path] = { source: 'override', revision: structuredClone(explicit.at(-1)!.layer.ref) };
    }
  }
  const parsed = check(values);
  return immutable({ values: parsed, sources, revisions: layers.map(layer => structuredClone(layer.ref)), definitionRevision: module.definitionRevision });
}

export function patchSparse(module: SettingsModule, scope: ConfigScope, sparse: JsonObject, patch: PatchOperation[]): JsonObject {
  assertJson(patch);
  for (const operation of patch) {
    assertKnown(module, operation.path);
    if ('from' in operation) assertKnown(module, operation.from);
  }
  try {
    const result = jsonpatch.applyPatch(structuredClone(sparse), structuredClone(patch), true, true, true).newDocument;
    assertSparse(module, result);
    assertChangesWritable(module, scope, sparse, result);
    return result;
  } catch (error) {
    if (error instanceof Error && 'problem' in error) throw error;
    // JSON-Patch errors contain the entire document and patch; never retain that raw error.
    throw problem('settings.invalid_patch', 'validation', 'JSON Patch无法应用到当前稀疏覆盖');
  }
}
export function resetSparse(module: SettingsModule, scope: ConfigScope, sparse: JsonObject, paths?: string[]): { sparse: JsonObject; patch: PatchOperation[] } {
  const selected = paths ?? [''];
  const patch: PatchOperation[] = [];
  let next = structuredClone(sparse);
  for (const path of selected) {
    assertKnown(module, path);
    if (path === '') { next = {}; patch.push({ op: 'replace', path: '', value: {} }); }
    else if (at(next, path).found) {
      const operation = { op: 'remove' as const, path };
      next = jsonpatch.applyPatch(next, [operation], true, true, true).newDocument;
      patch.push(operation);
    }
  }
  assertChangesWritable(module, scope, sparse, next);
  return { sparse: next, patch };
}
