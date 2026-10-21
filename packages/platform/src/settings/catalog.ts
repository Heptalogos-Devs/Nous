// Adapted from Nous-Wave 412b0620 apps/nous-core/src/configuration/catalog.ts
// and crates/configuration/src/snapshot.rs. Original code Apache-2.0.
// Copyright 2026 Aravine Zhu
import { createHash } from 'node:crypto';
import { canonicalize } from 'json-canonicalize';
import { z } from 'zod';
import { problem } from '../errors.js';
import { FieldMetadataSchema } from './types.js';
import type { ConfigScope, Json, JsonObject, ModuleDescription, SettingsModule } from './types.js';

export function digest(value: unknown): string {
  return createHash('sha256').update(canonicalize(value)).digest('hex');
}
export function immutable<T>(value: T): T {
  if (value && typeof value === 'object') {
    for (const child of Object.values(value)) immutable(child);
    Object.freeze(value);
  }
  return value;
}
export function isObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}
export function assertJson(value: unknown): asserts value is Json {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return;
  if (typeof value === 'number' && Number.isFinite(value)) return;
  if (Array.isArray(value)) { for (const child of value) assertJson(child); return; }
  if (isObject(value)) {
    for (const [key, child] of Object.entries(value)) {
      if (['__proto__', 'constructor', 'prototype'].includes(key)) throw problem('settings.invalid_path', 'validation', '设置键无效');
      assertJson(child);
    }
    return;
  }
  throw problem('settings.invalid_json', 'validation', '设置必须是有限JSON值');
}
export function parts(pointer: string): string[] {
  if (pointer !== '' && !/^\/(?:[^~]|~[01])*$/.test(pointer)) throw problem('settings.invalid_path', 'validation', '设置路径必须是JSON Pointer');
  const result = pointer === '' ? [] : pointer.slice(1).split('/').map(part => part.replaceAll('~1', '/').replaceAll('~0', '~'));
  if (result.some(part => ['__proto__', 'constructor', 'prototype'].includes(part))) throw problem('settings.invalid_path', 'validation', '设置路径无效');
  return result;
}
export const pointer = (path: readonly string[]) => path.length ? '/' + path.map(part => part.replaceAll('~', '~0').replaceAll('/', '~1')).join('/') : '';
export const within = (path: string, parent: string) => parent === '' || path === parent || path.startsWith(parent + '/');
export function at(value: Json, path: string): { found: boolean; value?: Json } {
  let current: Json = value;
  for (const key of parts(path)) {
    if (!current || typeof current !== 'object' || !Object.hasOwn(current, key)) return { found: false };
    current = (current as JsonObject)[key];
  }
  return { found: true, value: current };
}
export function leaves(value: Json, path: string[] = []): string[] {
  if (!isObject(value) || !Object.keys(value).length) return [pointer(path)];
  return Object.entries(value).flatMap(([key, child]) => leaves(child, [...path, key]));
}
export function scopeId(scope: ConfigScope): string {
  if (scope.kind === 'installation') return scope.installationId;
  if (scope.kind === 'subject') return scope.subjectId;
  return canonicalize([scope.subjectId, scope.roleId]);
}
export const scopeKey = (scope: ConfigScope) => canonicalize([scope.kind, scopeId(scope)]);
export function scopes(installationId: string, scope: ConfigScope): ConfigScope[] {
  if (scope.kind === 'installation' && scope.installationId !== installationId) throw problem('settings.installation_mismatch', 'validation', '设置安装标识不匹配');
  const result: ConfigScope[] = [{ kind: 'installation', installationId }];
  if (scope.kind !== 'installation') result.push({ kind: 'subject', subjectId: scope.subjectId });
  if (scope.kind === 'role') result.push(scope);
  return result;
}
export function parse<T>(schema: z.ZodType<T>, input: unknown): T {
  const result = schema.safeParse(input);
  if (!result.success) throw problem('settings.invalid_value', 'validation', '设置值无效', {
    details: { issues: result.error.issues.map(issue => ({ path: pointer(issue.path.map(String)), code: issue.code })) },
  });
  return result.data;
}

/** Owner fields remain explicit; no traversal of a schema's private representation. */
export function createCatalog(modules: readonly SettingsModule[]) {
  const descriptions = new Map<string, ModuleDescription>();
  const executable = new Map<string, SettingsModule>();
  for (const module of modules) {
    if (!module.id || module.id.length > 256 || !module.definitionRevision || !module.owner || executable.has(module.id)) {
      throw problem('settings.invalid_definition', 'validation', '设置定义标识无效或重复');
    }
    assertJson(module.defaults);
    if (!isObject(module.defaults)) throw problem('settings.invalid_definition', 'validation', '设置默认值必须是对象');
    const fields = parse(z.record(z.string(), FieldMetadataSchema), module.fields);
    for (const path of Object.keys(fields)) parts(path);
    const defaults = parse(module.schema, structuredClone(module.defaults));
    assertJson(defaults);
    if (digest(defaults) !== digest(module.defaults)) throw problem('settings.invalid_definition', 'validation', '设置schema必须保留完整默认值，默认值单独声明');
    const registered = { ...module, defaults: immutable(structuredClone(module.defaults)), fields: immutable(structuredClone(fields)) };
    const description = {
      namespace: module.id, definitionRevision: module.definitionRevision, owner: module.owner,
      jsonSchema: z.toJSONSchema(module.schema) as JsonObject,
      defaults: structuredClone(module.defaults), fields: structuredClone(fields),
    };
    assertJson(description.jsonSchema);
    for (const leaf of leaves(module.defaults)) assertKnown(registered, leaf);
    descriptions.set(module.id, immutable(description));
    executable.set(module.id, Object.freeze(registered));
  }
  return {
    ids: Object.freeze([...executable.keys()]),
    module(namespace: string) {
      const module = executable.get(namespace);
      if (!module) throw problem('settings.namespace_not_found', 'not_found', '设置命名空间不存在', { details: { namespace } });
      return module;
    },
    description(namespace: string) {
      this.module(namespace);
      return descriptions.get(namespace)!;
    },
  };
}

export function assertKnown(module: SettingsModule, path: string): void {
  parts(path);
  if (!Object.keys(module.fields).some(field => within(path, field) || within(field, path))) {
    throw problem('settings.unknown_field', 'validation', '设置字段未声明', { details: { path } });
  }
}
export function assertSparse(module: SettingsModule, value: unknown): asserts value is JsonObject {
  assertJson(value);
  if (!isObject(value)) throw problem('settings.invalid_value', 'validation', '稀疏设置必须是对象');
  for (const path of leaves(value)) { if (path !== '') assertKnown(module, path); }
}
export function assertWritable(module: SettingsModule, scope: ConfigScope, path: string): void {
  assertKnown(module, path);
  for (const [field, metadata] of Object.entries(module.fields)) {
    if (!(within(path, field) || within(field, path))) continue;
    if (metadata.readonly || !metadata.scopes.includes(scope.kind)) {
      throw problem('settings.field_forbidden', 'forbidden', '此范围不能修改设置字段', { details: { path: field, scope: scope.kind } });
    }
  }
}

/** Check the changed leaves of a container, rather than rejecting an unrelated readonly sibling. */
export function assertChangesWritable(module: SettingsModule, scope: ConfigScope, before: JsonObject, after: JsonObject): void {
  for (const [path, metadata] of Object.entries(module.fields)) {
    if (digest(at(before, path)) === digest(at(after, path))) continue;
    if (metadata.readonly || !metadata.scopes.includes(scope.kind)) throw problem('settings.field_forbidden', 'forbidden', '此范围不能修改设置字段', { details: { path, scope: scope.kind } });
  }
}
