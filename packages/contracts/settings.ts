// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: AGPL-3.0-only
import { z } from 'zod';
import {
  JsonValueSchema,
  RefSchema,
  ActorSchema,
  InstantSchema,
  ProblemSchema,
  SubjectIdSchema,
} from './common.js';
import type { CallContext, Problem, Ref } from './common.js';

export type Json =
  | null
  | boolean
  | number
  | string
  | Json[]
  | { [key: string]: Json };
export type JsonObject = { [key: string]: Json };
export type ConfigScope =
  | { kind: 'installation'; installationId: string }
  | { kind: 'subject'; subjectId: string }
  | { kind: 'role'; subjectId: string; roleId: string };
export const ConfigScopeSchema = z.discriminatedUnion('kind', [
  z.strictObject({
    kind: z.literal('installation'),
    installationId: z.string().min(1).max(512),
  }),
  z.strictObject({ kind: z.literal('subject'), subjectId: SubjectIdSchema }),
  z.strictObject({
    kind: z.literal('role'),
    subjectId: SubjectIdSchema,
    roleId: z.string().min(1).max(512),
  }),
]);
export interface FieldMetadata {
  labelKey: string;
  descriptionKey: string;
  scopes: ConfigScope['kind'][];
  visibility: 'standard' | 'advanced' | 'developer';
  effect: 'hot' | 'next_call' | 'restart';
  unit?: string;
  readonly?: boolean;
  restriction?: 'upper_bound' | 'set_intersection';
  consumerIds: string[];
}
export const FieldMetadataSchema = z.strictObject({
  labelKey: z.string().min(1),
  descriptionKey: z.string().min(1),
  scopes: z.array(z.enum(['installation', 'subject', 'role'])).min(1),
  visibility: z.enum(['standard', 'advanced', 'developer']),
  effect: z.enum(['hot', 'next_call', 'restart']),
  unit: z.string().optional(),
  readonly: z.boolean().optional(),
  restriction: z.enum(['upper_bound', 'set_intersection']).optional(),
  consumerIds: z.array(z.string().min(1)),
});
export interface ModuleDescription {
  namespace: string;
  definitionRevision: string;
  owner: string;
  jsonSchema: JsonObject;
  defaults: JsonObject;
  fields: Record<string, FieldMetadata>;
  recordedAt?: string;
}
export interface RevisionRef {
  namespace: string;
  scope: ConfigScope;
  revision: number;
}
export interface ConfigSnapshot<T = JsonObject> {
  values: Readonly<T>;
  sources: Record<
    string,
    { source: 'default' | 'override'; revision?: RevisionRef }
  >;
  revisions: RevisionRef[];
  definitionRevision: string;
}
export interface EffectiveSettingsView {
  scope: ConfigScope;
  modules: Record<string, ConfigSnapshot>;
  targetOverrides: Record<string, { revision: number; sparse: JsonObject }>;
}
export type PatchOperation =
  | { op: 'add' | 'replace' | 'test'; path: string; value: Json }
  | { op: 'remove'; path: string }
  | { op: 'move' | 'copy'; path: string; from: string };
const PointerSchema = z
  .string()
  .refine(
    (value) => value === '' || /^\/(?:[^~]|~[01])*$/.test(value),
    '路径必须是RFC 6901 JSON Pointer',
  );
export const PatchOperationSchema = z.discriminatedUnion('op', [
  z.strictObject({
    op: z.literal('add'),
    path: PointerSchema,
    value: JsonValueSchema,
  }),
  z.strictObject({
    op: z.literal('replace'),
    path: PointerSchema,
    value: JsonValueSchema,
  }),
  z.strictObject({
    op: z.literal('test'),
    path: PointerSchema,
    value: JsonValueSchema,
  }),
  z.strictObject({ op: z.literal('remove'), path: PointerSchema }),
  z.strictObject({
    op: z.literal('move'),
    path: PointerSchema,
    from: PointerSchema,
  }),
  z.strictObject({
    op: z.literal('copy'),
    path: PointerSchema,
    from: PointerSchema,
  }),
]);
const changeShape = {
  namespace: z.string().min(1).max(256),
  scope: ConfigScopeSchema,
  expectedRevision: z.number().int().nonnegative().safe(),
  reason: z.string().max(4096).optional(),
};
export const SettingsChangeSchema = z.strictObject({
  ...changeShape,
  patch: z.array(PatchOperationSchema),
});
export const SettingsResetSchema = z.strictObject({
  ...changeShape,
  paths: z.array(PointerSchema).optional(),
});
export const SettingsRestoreSchema = z.strictObject({
  ...changeShape,
  revision: z.number().int().positive().safe(),
});
export const SettingsReadSchema = z.strictObject({
  namespaces: z.array(changeShape.namespace),
  scope: ConfigScopeSchema,
});
export const SettingsDescribeSchema = z.strictObject({
  namespace: changeShape.namespace.optional(),
  scope: ConfigScopeSchema.optional(),
  definitionRevision: z.string().min(1).optional(),
});
export const SettingsHistorySchema = z.strictObject({
  namespace: changeShape.namespace,
  scope: ConfigScopeSchema,
  cursor: z.string().max(4096).optional(),
  limit: z.number().int().min(1).max(200).optional(),
});
export type SettingsChange = z.infer<typeof SettingsChangeSchema>;
export type SettingsReset = z.infer<typeof SettingsResetSchema>;
export type SettingsRestore = z.infer<typeof SettingsRestoreSchema>;
export interface SettingsChangeResult {
  namespace: string;
  scope: ConfigScope;
  operationId: string;
  revision: number;
  changed: boolean;
  snapshot: ConfigSnapshot;
  effects: Record<FieldMetadata['effect'], string[]>;
}
export interface SettingsRevisionOrigin {
  scope: ConfigScope;
  revision: number;
  actor: CallContext['actor'];
  recordedAt: string;
  definitionRevision: string;
  sourceBundleRef?: Ref;
  previous?: SettingsRevisionOrigin;
}
export interface SettingsRevision {
  namespace: string;
  scope: ConfigScope;
  revision: number;
  definitionRevision: string;
  sparse: JsonObject;
  patch: PatchOperation[];
  actor: CallContext['actor'];
  reason?: string;
  recordedAt: string;
  definition: ModuleDescription;
  origin?: SettingsRevisionOrigin;
}
export interface SettingsNotification {
  namespace: string;
  scope: ConfigScope;
  revision: number;
}
export type SettingsEvent =
  | { kind: 'changed'; change: SettingsNotification }
  | { kind: 'resync' };
export type SettingsAction =
  | 'describe'
  | 'read'
  | 'update'
  | 'reset'
  | 'history'
  | 'restore'
  | 'applied'
  | 'export'
  | 'import'
  | 'deleteScopes';
export interface AuthorizationRequest {
  action: SettingsAction;
  namespace: string;
  scope: ConfigScope;
  paths?: readonly string[];
}
export interface AppliedField {
  definitionRevision: string;
  revisions: RevisionRef[];
  value?: Json;
}
export interface ConsumerAppliedState {
  processBootId: string;
  consumerId: string;
  namespace: string;
  scope: ConfigScope;
  fields: Record<string, AppliedField>;
  error?: Problem;
}
export interface ConsumerAppliedView
  extends Omit<ConsumerAppliedState, 'fields'> {
  fields: Record<
    string,
    {
      effect: FieldMetadata['effect'];
      state: 'current' | 'pending' | 'unknown' | 'failed';
      desired: AppliedField;
      applied?: AppliedField;
    }
  >;
}

const JsonObjectSchema = z.record(z.string(), JsonValueSchema);
export const RevisionRefSchema = z.strictObject({
  namespace: changeShape.namespace,
  scope: ConfigScopeSchema,
  revision: changeShape.expectedRevision,
});
export const ConfigSnapshotSchema = z.strictObject({
  values: JsonObjectSchema,
  sources: z.record(
    z.string(),
    z.strictObject({
      source: z.enum(['default', 'override']),
      revision: RevisionRefSchema.optional(),
    }),
  ),
  revisions: z.array(RevisionRefSchema),
  definitionRevision: z.string().min(1),
});
export const EffectiveSettingsViewSchema = z.strictObject({
  scope: ConfigScopeSchema,
  modules: z.record(z.string(), ConfigSnapshotSchema),
  targetOverrides: z.record(
    z.string(),
    z.strictObject({
      revision: changeShape.expectedRevision,
      sparse: JsonObjectSchema,
    }),
  ),
});
export const ModuleDescriptionSchema = z.strictObject({
  namespace: changeShape.namespace,
  definitionRevision: z.string().min(1),
  owner: z.string().min(1),
  jsonSchema: JsonObjectSchema,
  defaults: JsonObjectSchema,
  fields: z.record(z.string(), FieldMetadataSchema),
  recordedAt: InstantSchema.optional(),
});
export const SettingsChangeResultSchema = z.strictObject({
  namespace: changeShape.namespace,
  scope: ConfigScopeSchema,
  operationId: z.string().min(1),
  revision: changeShape.expectedRevision,
  changed: z.boolean(),
  snapshot: ConfigSnapshotSchema,
  effects: z.strictObject({
    hot: z.array(z.string()),
    next_call: z.array(z.string()),
    restart: z.array(z.string()),
  }),
});
export const SettingsRevisionOriginSchema: z.ZodType<SettingsRevisionOrigin> =
  z.lazy(() =>
    z.strictObject({
      scope: ConfigScopeSchema,
      revision: z.number().int().positive().safe(),
      actor: ActorSchema,
      recordedAt: InstantSchema,
      definitionRevision: z.string().min(1),
      sourceBundleRef: RefSchema.optional(),
      previous: SettingsRevisionOriginSchema.optional(),
    }),
  );
export const SettingsRevisionSchema = z.strictObject({
  namespace: changeShape.namespace,
  scope: ConfigScopeSchema,
  revision: z.number().int().positive().safe(),
  definitionRevision: z.string().min(1),
  sparse: JsonObjectSchema,
  patch: z.array(PatchOperationSchema),
  actor: ActorSchema,
  reason: z.string().optional(),
  recordedAt: InstantSchema,
  definition: ModuleDescriptionSchema,
  origin: SettingsRevisionOriginSchema.optional(),
});
export const SettingsHistoryPageSchema = z.strictObject({
  items: z.array(SettingsRevisionSchema),
  nextCursor: z.string().optional(),
});
export const AppliedFieldSchema = z.strictObject({
  definitionRevision: z.string().min(1),
  revisions: z.array(RevisionRefSchema),
  value: JsonValueSchema.optional(),
});
export const ConsumerAppliedViewSchema = z.strictObject({
  processBootId: z.string().min(1),
  consumerId: z.string().min(1),
  namespace: changeShape.namespace,
  scope: ConfigScopeSchema,
  fields: z.record(
    z.string(),
    z.strictObject({
      effect: z.enum(['hot', 'next_call', 'restart']),
      state: z.enum(['current', 'pending', 'unknown', 'failed']),
      desired: AppliedFieldSchema,
      applied: AppliedFieldSchema.optional(),
    }),
  ),
  error: ProblemSchema.optional(),
});



export const SettingsBundleSchema = z.strictObject({
  formatVersion: z.literal(1),
  historyComplete: z.boolean(),
  sourceBundleRef: RefSchema.optional(),
  definitions: z.array(ModuleDescriptionSchema).max(1000),
  heads: z
    .array(
      z.strictObject({
        namespace: z.string().min(1),
        scope: ConfigScopeSchema,
        revision: z.number().int().positive().safe(),
      }),
    )
    .max(10000),
  revisions: z
    .array(SettingsRevisionSchema.omit({ definition: true }))
    .max(100000),
});
export type SettingsBundle = z.infer<typeof SettingsBundleSchema>;
export const SettingsScopeMapSchema = z
  .array(
    z.strictObject({
      from: ConfigScopeSchema,
      to: ConfigScopeSchema,
      expectedRevisions: z.record(
        z.string(),
        z.number().int().nonnegative().safe(),
      ),
    }),
  )
  .max(1000);
export type SettingsScopeMap = z.infer<typeof SettingsScopeMapSchema>;

for (const [id, schema] of Object.entries({
  ConfigScopeSchema,
  FieldMetadataSchema,
  PatchOperationSchema,
  SettingsChangeSchema,
  SettingsResetSchema,
  SettingsRestoreSchema,
  SettingsReadSchema,
  SettingsDescribeSchema,
  SettingsHistorySchema,
  RevisionRefSchema,
  ConfigSnapshotSchema,
  EffectiveSettingsViewSchema,
  ModuleDescriptionSchema,
  SettingsChangeResultSchema,
  SettingsRevisionSchema,
  SettingsRevisionOriginSchema,SettingsBundleSchema,SettingsScopeMapSchema,
  SettingsHistoryPageSchema,
  AppliedFieldSchema,
  ConsumerAppliedViewSchema,
}))
  z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });
