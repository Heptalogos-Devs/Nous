// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { z } from 'zod';
import {
  ActorSchema,
  ConsumerSchema,
  RefSchema,
  SubjectIdSchema,
  TimePointInputSchema,
  TimePointSchema,
  type Actor,
  type Consumer,
  type Ref,
} from './common.js';

export const AuthorityPlaneSchema = z.enum([
  'subject',
  'system',
  'machine',
  'recovery',
]);
export type AuthorityPlane = z.infer<typeof AuthorityPlaneSchema>;
const actions = z.array(z.string().min(1)).min(1);
export const TargetScopeSchema = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('conversation'),
    adapterId: z.string().min(1),
    accountId: z.string().min(1),
    conversationId: z.string().min(1),
    actions,
  }),
  z.object({
    kind: z.literal('resource'),
    connectorId: z.string().min(1),
    resourceIds: z.array(z.string().min(1)).min(1),
    actions,
  }),
  z.object({ kind: z.literal('workspace'), workspaceRef: RefSchema, actions }),
  z.object({
    kind: z.literal('object'),
    refs: z.array(RefSchema).min(1),
    actions,
  }),
]);
export type TargetScope = z.infer<typeof TargetScopeSchema>;
export const DelegationScopeSchema = z
  .object({
    plane: AuthorityPlaneSchema,
    operations: z.array(z.string().min(1)).min(1),
    subjectIds: z.array(SubjectIdSchema),
    consumer: ConsumerSchema,
    purposes: z.array(z.string().min(1)).min(1),
    audiences: z.array(z.string().min(1)),
    targets: z.array(TargetScopeSchema),
    inputRefs: z.array(RefSchema),
    taskRef: RefSchema.optional(),
    nativeBinding: z
      .object({
        runtimeId: z.string().min(1),
        sessionKey: z.string().min(1),
        rootTaskId: z.string().optional(),
      })
      .optional(),
    notBefore: TimePointInputSchema.optional(),
    expiresAt: TimePointInputSchema.nullable(),
    mayDelegate: z.boolean(),
  })
  .superRefine((value, ctx) => {
    if (value.operations.some((op) => op.includes('*')))
      ctx.addIssue({
        code: 'custom',
        message: '操作权限必须是目录中的明确操作',
      });
    if (value.consumer.kind !== 'maintenance' && value.plane !== 'subject')
      ctx.addIssue({
        code: 'custom',
        message: '主体、工作者和作者消费必须使用主体权限平面',
      });
    if (
      (value.expiresAt && value.expiresAt.domainId !== 'real') ||
      (value.notBefore && value.notBefore.domainId !== 'real')
    )
      ctx.addIssue({ code: 'custom', message: '委派有效期必须使用real时域' });
    if (
      (value.plane === 'subject' ||
        value.consumer.kind === 'worker' ||
        value.consumer.kind === 'subject') &&
      value.subjectIds.length === 0
    )
      ctx.addIssue({ code: 'custom', message: '主体与工作者委派必须绑定主体' });
  });
export type DelegationScope = z.infer<typeof DelegationScopeSchema>;
export interface ServiceDelegation {
  id: string;
  apiKeyId: string | null;
  ownerUserId: string;
  parentAuthorityRef: string;
  issuerAuthorityRef: string;
  principalKind: Actor['kind'];
  scope: DelegationScope;
  revision: number;
  state: 'pending' | 'active' | 'revoked';
}

export const SecretRefSchema = z
  .string()
  .regex(
    /^secret:[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
export const SecretBackendSchema = z.enum([
  'environment',
  'os',
  'protected_file',
]);
export type SecretBackendInput =
  | { kind: 'environment'; variable: string }
  | { kind: 'os' | 'protected_file'; material: Uint8Array };
export interface SecretCreate {
  purpose: string;
  resourceRef: Ref;
  allowedConsumers: Consumer[];
  backend: SecretBackendInput;
}
export interface SecretRotate {
  ref: string;
  expectedRevision: number;
  backend: SecretBackendInput;
  allowedConsumers?: Consumer[];
}
export interface SecretRevoke {
  ref: string;
  expectedRevision: number;
}
export interface SecretUseMetadata {
  readonly ref: string;
  readonly revision: number;
  readonly generation: number;
}
export const SecretUseSchema = z.strictObject({
  ref: SecretRefSchema,
  purpose: z.string().min(1).max(256),
  resourceRef: RefSchema.refine((ref) => !ref.selector),
  useRef: RefSchema.refine((ref) => !ref.selector).optional(),
});
export type SecretUse = z.infer<typeof SecretUseSchema>;
export const SecretStatusSchema = z.object({
  ref: SecretRefSchema,
  revision: z.number().int().positive(),
  generation: z.number().int().positive(),
  state: z.enum(['active', 'revoked', 'unavailable']),
  purpose: z.string(),
  resourceRef: RefSchema,
  allowedConsumers: z.array(ConsumerSchema),
  backend: SecretBackendSchema,
  configured: z.boolean(),
  available: z.boolean(),
  createdAt: TimePointSchema,
  replacedAt: TimePointSchema.optional(),
  revokedAt: TimePointSchema.optional(),
  cleanupPending: z.boolean(),
  externalRotationRequired: z.boolean(),
});
export type SecretStatus = z.infer<typeof SecretStatusSchema>;

export const AuthenticatedIdentitySchema = z
  .strictObject({ actor: ActorSchema, consumer: ConsumerSchema })
  .meta({ id: 'AuthenticatedIdentity' });
export type AuthenticatedIdentity = z.infer<typeof AuthenticatedIdentitySchema>;

export const DelegationChildCreateSchema = z.strictObject({
  name: z.string().min(1).max(256),
  scope: DelegationScopeSchema,
  principalKind: ActorSchema.shape.kind,
});
export const DelegationChildViewSchema = z.strictObject({
  delegationId: z.uuidv7(),
  authorityRef: z.string().regex(/^delegation:[0-9a-f-]{36}$/),
  revision: z.number().int().positive(),
  scope: DelegationScopeSchema,
});
export type DelegationChildCreate = z.infer<typeof DelegationChildCreateSchema>;
export type DelegationChildView = z.infer<typeof DelegationChildViewSchema>;
z.globalRegistry.add(DelegationChildCreateSchema, {
  id: 'DelegationChildCreateBody',
});
z.globalRegistry.add(DelegationChildViewSchema, { id: 'DelegationChildView' });
