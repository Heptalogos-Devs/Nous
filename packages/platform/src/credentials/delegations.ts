import { createHash } from 'node:crypto';
import {
  type Actor,
  type CallContext,
  type Consumer,
  ConsumerSchema,
  type Ref,
  RefSchema,
  SubjectIdSchema,
  TimePointInputSchema,
} from '@nous-wave/contracts/common';
import { canonicalize } from 'json-canonicalize';
import { v7 as uuid } from 'uuid';
// Scope matching adapted from Heptalogos policy-service.ts, commit
// 21f0080460f3ee639deb9f5ea47282fb6c59df95. AGPL-3.0-only.
import { z } from 'zod';
import { problem } from '../errors.js';
import {
  type ColumnType,
  type Kysely,
  type Migration,
  sql,
} from '../storage/index.js';
import { readReal } from '../time/index.js';

export type {
  AuthorityPlane,
  DelegationScope,
  ServiceDelegation,
  TargetScope,
} from '@nous-wave/contracts/credentials';
export {
  AuthorityPlaneSchema,
  DelegationScopeSchema,
  TargetScopeSchema,
} from '@nous-wave/contracts/credentials';

import {
  type AuthorityPlane,
  type DelegationScope,
  DelegationScopeSchema,
  type ServiceDelegation,
  type TargetScope,
} from '@nous-wave/contracts/credentials';
export interface DelegationDatabase {
  serviceDelegations: {
    id: string;
    apiKeyId: string | null;
    ownerUserId: string;
    parentAuthorityRef: string;
    issuerAuthorityRef: string;
    principalKind: Actor['kind'];
    scope: ColumnType<
      DelegationScope,
      string | DelegationScope,
      string | DelegationScope
    >;
    revision: number;
    state: 'pending' | 'active' | 'revoked';
    createdAt: ColumnType<string, string | undefined, never>;
    revokedAt: string | null;
    submissionKey: string | null;
    requestDigest: string | null;
  };
}
export const delegationMigration: Migration = {
  async up(db) {
    await sql`create table if not exists service_delegations(id uuid primary key,api_key_id text unique,owner_user_id text not null,parent_authority_ref text not null,issuer_authority_ref text not null,principal_kind text not null,scope jsonb not null,revision integer not null check(revision>0),state text not null check(state in ('pending','active','revoked')),created_at timestamptz not null default clock_timestamp(),revoked_at timestamptz,submission_key text unique,request_digest text)`.execute(
      db,
    );
  },
  async down(db) {
    await db.schema.dropTable('serviceDelegations').execute();
  },
};
export interface OperationPolicy {
  allowedPlanes: AuthorityPlane[];
  targetRequired?: boolean;
}
export interface CredentialState {
  userId: string;
  enabled: boolean;
  expiresAt?: string | null;
}
export interface DelegationOptions {
  db: Kysely<any>;
  product: 'hept' | 'nous';
  installationId: string;
  operationPolicy: (operation: string) => OperationPolicy | undefined;
  currentUser: (
    id: string,
    database?: Kysely<any>,
  ) => Promise<{ id: string; admin: boolean; enabled: boolean } | null>;
  currentKey: (
    id: string,
    database?: Kysely<any>,
  ) => Promise<CredentialState | null>;
  currentSession: (
    id: string,
    database?: Kysely<any>,
  ) => Promise<{ userId: string; expiresAt: string } | null>;
  createKey: (
    userId: string,
    input: { name: string; expiresIn: number | null },
    database?: Kysely<any>,
  ) => Promise<{ id: string; key: string }>;
  disableKey: (id: string, database?: Kysely<any>) => Promise<void>;
  sourceGuard?: (
    ctx: CallContext,
    operation: string,
    target?: Ref | TargetScope,
    database?: Kysely<any>,
  ) => Promise<void>;
  issuancePolicy?: (
    input: {
      issuer: { actor: Actor; consumer: Consumer; scope?: DelegationScope };
      requested: DelegationScope;
      requestedPrincipalKind: Actor['kind'];
    },
    database?: Kysely<any>,
  ) => Promise<boolean> | boolean;
}
function equal(a: unknown, b: unknown) {
  return canonicalize(a) === canonicalize(b);
}
function permitted(child: string[], parent: string[]) {
  return parent.includes('*') || child.every((item) => parent.includes(item));
}
function subset(child: string[], parent: string[]) {
  return child.every((item) => parent.includes(item));
}
function refContains(parent: Ref, child: Ref): boolean {
  return (
    parent.product === child.product &&
    parent.installationId === child.installationId &&
    parent.subjectId === child.subjectId &&
    parent.kind === child.kind &&
    parent.id === child.id &&
    (parent.revision === undefined || parent.revision === child.revision) &&
    (parent.selector === undefined || equal(parent.selector, child.selector))
  );
}
function targetContains(parent: TargetScope, child: TargetScope): boolean {
  if (parent.kind !== child.kind || !subset(child.actions, parent.actions))
    return false;
  if (parent.kind === 'object' && child.kind === 'object')
    return child.refs.every((ref) =>
      parent.refs.some((p) => refContains(p, ref)),
    );
  if (parent.kind === 'workspace' && child.kind === 'workspace')
    return refContains(parent.workspaceRef, child.workspaceRef);
  if (parent.kind === 'resource' && child.kind === 'resource')
    return (
      parent.connectorId === child.connectorId &&
      subset(child.resourceIds, parent.resourceIds)
    );
  return (
    parent.kind === 'conversation' &&
    child.kind === 'conversation' &&
    parent.adapterId === child.adapterId &&
    parent.accountId === child.accountId &&
    parent.conversationId === child.conversationId
  );
}
function targetBound(
  scope: DelegationScope,
  operation: string,
  policy: DelegationOptions['operationPolicy'],
): TargetScope[] | null {
  const relevant = scope.targets
    .filter((target) => target.actions.includes(operation))
    .map((target) => ({ ...target, actions: [operation] }));
  // null is unrestricted for this operation; [] has no positive target grant.
  return relevant.length
    ? relevant
    : scope.targets.length && policy(operation)?.targetRequired
      ? []
      : null;
}
function targetsContain(
  parent: DelegationScope,
  child: DelegationScope,
  policy: DelegationOptions['operationPolicy'],
): boolean {
  return child.operations.every((operation) => {
    const allowed = targetBound(parent, operation, policy);
    const requested = targetBound(child, operation, policy);
    return (
      allowed === null ||
      (requested !== null &&
        requested.every((target) =>
          allowed.some((bound) => targetContains(bound, target)),
        ))
    );
  });
}
function expiresWithin(child: DelegationScope, parent: DelegationScope) {
  return (
    !parent.expiresAt ||
    Boolean(child.expiresAt && child.expiresAt.at <= parent.expiresAt.at)
  );
}
function scopeContains(
  parent: DelegationScope,
  child: DelegationScope,
  policy: DelegationOptions['operationPolicy'],
): boolean {
  return (
    parent.plane === child.plane &&
    equal(parent.consumer, child.consumer) &&
    subset(child.operations, parent.operations) &&
    subset(child.subjectIds, parent.subjectIds) &&
    permitted(child.purposes, parent.purposes) &&
    permitted(child.audiences, parent.audiences) &&
    targetsContain(parent, child, policy) &&
    child.inputRefs.every((ref) =>
      parent.inputRefs.some((p) => refContains(p, ref)),
    ) &&
    (!parent.taskRef ||
      Boolean(child.taskRef && refContains(parent.taskRef, child.taskRef))) &&
    (!parent.nativeBinding ||
      equal(parent.nativeBinding, child.nativeBinding)) &&
    expiresWithin(child, parent) &&
    (!parent.notBefore ||
      Boolean(child.notBefore && child.notBefore.at >= parent.notBefore.at)) &&
    (!child.mayDelegate || parent.mayDelegate)
  );
}
function principalId(value: ServiceDelegation): string {
  return value.principalKind === 'human'
    ? value.ownerUserId
    : ['subject', 'worker', 'author'].includes(value.principalKind)
      ? value.scope.consumer.id
      : value.id;
}
export function createDelegations(options: DelegationOptions) {
  const { db } = options;
  function transaction<T>(
    database: Kysely<any> | undefined,
    use: (tx: Kysely<any>) => Promise<T>,
  ) {
    if (!database) return db.transaction().execute(use);
    if (!database.isTransaction)
      throw problem(
        'credentials.transaction_required',
        'validation',
        '凭据操作需要当前业务事务',
      );
    return use(database);
  }
  function deny(): never {
    throw problem(
      'credentials.scope_denied',
      'forbidden',
      '当前身份没有这项操作权限',
    );
  }
  function checkedScope(input: DelegationScope) {
    const scope = DelegationScopeSchema.parse(input);
    for (const op of scope.operations) {
      const policy = options.operationPolicy(op);
      if (!policy || !policy.allowedPlanes.includes(scope.plane)) deny();
    }
    if (scope.expiresAt && scope.expiresAt.at <= readReal().at) deny();
    return scope;
  }
  async function get(
    id: string,
    database?: Kysely<any>,
  ): Promise<(ServiceDelegation & { submissionKey: string | null }) | null> {
    const result = await sql<
      ServiceDelegation & { submissionKey: string | null }
    >`select id,api_key_id,owner_user_id,parent_authority_ref,issuer_authority_ref,principal_kind,scope,revision,state,submission_key from service_delegations where id=${id}`.execute(
      database ?? db,
    );
    return result.rows[0] ?? null;
  }
  async function active(
    id: string,
    database?: Kysely<any>,
    seen = new Set<string>(),
  ): Promise<ServiceDelegation> {
    if (seen.has(id) || seen.size >= 32) deny();
    seen.add(id);
    const value = await get(id, database);
    if (!value || value.state !== 'active' || !value.apiKeyId) deny();
    const now = readReal().at,
      scope = DelegationScopeSchema.parse(value.scope);
    if (
      (scope.expiresAt && scope.expiresAt.at <= now) ||
      (scope.notBefore && scope.notBefore.at > now)
    )
      deny();
    const key = await options.currentKey(value.apiKeyId, database),
      user = await options.currentUser(value.ownerUserId, database);
    if (
      !key?.enabled ||
      key.userId !== value.ownerUserId ||
      (key.expiresAt && key.expiresAt <= now) ||
      !user?.enabled
    )
      deny();
    if (value.parentAuthorityRef.startsWith('delegation:')) {
      const parent = await active(
        value.parentAuthorityRef.slice(11),
        database,
        seen,
      );
      if (
        value.submissionKey &&
        (parent.principalKind !== 'service' ||
          !parent.scope.operations.includes('delegations.select'))
      )
        deny();
      if (
        !parent.scope.mayDelegate ||
        !parent.scope.operations.includes('delegations.create') ||
        !expiresWithin(scope, parent.scope) ||
        !targetsContain(parent.scope, scope, options.operationPolicy) ||
        (parent.scope.notBefore &&
          (!scope.notBefore || scope.notBefore.at < parent.scope.notBefore.at))
      )
        deny();
      if (
        !(
          scopeContains(parent.scope, scope, options.operationPolicy) &&
          (parent.principalKind === 'human' ||
            parent.principalKind === value.principalKind)
        ) &&
        !(await options.issuancePolicy?.(
          {
            issuer: {
              actor: {
                id: principalId(parent),
                kind: parent.principalKind,
                authorityRef: value.parentAuthorityRef,
              },
              consumer: parent.scope.consumer,
              scope: parent.scope,
            },
            requested: scope,
            requestedPrincipalKind: value.principalKind,
          },
          database,
        ))
      )
        deny();
    } else {
      const session = value.parentAuthorityRef.startsWith('session:')
        ? await options.currentSession(
            value.parentAuthorityRef.slice(8),
            database,
          )
        : null;
      if (
        (value.parentAuthorityRef !== `user:${user.id}` &&
          (!session ||
            session.userId !== user.id ||
            session.expiresAt <= now ||
            !scope.expiresAt ||
            scope.expiresAt.at > session.expiresAt)) ||
        !user.admin
      )
        deny();
      const local =
        scope.plane === 'system' &&
        scope.subjectIds.length === 0 &&
        equal(scope.consumer, {
          kind: 'maintenance',
          host: options.product,
          id: options.installationId,
        }) &&
        value.principalKind === 'human';
      if (
        !local &&
        !(await options.issuancePolicy?.(
          {
            issuer: {
              actor: {
                id: user.id,
                kind: 'human',
                authorityRef: value.parentAuthorityRef,
              },
              consumer: {
                kind: 'maintenance',
                host: options.product,
                id: options.installationId,
              },
            },
            requested: scope,
            requestedPrincipalKind: value.principalKind,
          },
          database,
        ))
      )
        deny();
    }
    return { ...value, scope };
  }
  async function authority(
    ctx: CallContext,
    database?: Kysely<any>,
  ): Promise<DelegationScope | null> {
    const ref = ctx.actor.authorityRef;
    if (ref.startsWith('user:') || ref.startsWith('session:')) {
      const session = ref.startsWith('session:')
        ? await options.currentSession(ref.slice(8), database)
        : null;
      if (
        ref.startsWith('session:') &&
        (!session || session.expiresAt <= readReal().at)
      )
        deny();
      const user = await options.currentUser(
        session?.userId ?? ref.slice(5),
        database,
      );
      if (
        !user?.enabled ||
        !user.admin ||
        ctx.actor.kind !== 'human' ||
        ctx.actor.id !== user.id ||
        !equal(ctx.consumer, {
          kind: 'maintenance',
          host: options.product,
          id: options.installationId,
        })
      )
        deny();
      return null;
    }
    if (!ref.startsWith('delegation:')) deny();
    const value = await active(ref.slice(11), database);
    if (
      ctx.actor.id !== principalId(value) ||
      ctx.actor.kind !== value.principalKind ||
      !equal(ctx.consumer, value.scope.consumer)
    )
      deny();
    return value.scope;
  }
  async function authorize(
    ctx: CallContext,
    operation: string,
    target?: Ref | TargetScope,
    database?: Kysely<any>,
  ) {
    const policy = options.operationPolicy(operation);
    if (!policy) deny();
    const scope = await authority(ctx, database);
    if (scope) {
      if (
        !policy.allowedPlanes.includes(scope.plane) ||
        !scope.operations.includes(operation) ||
        !permitted([ctx.purpose], scope.purposes) ||
        !permitted(ctx.audience, scope.audiences) ||
        (ctx.subjectId !== undefined &&
          !(scope.plane === 'system' && scope.subjectIds.length === 0) &&
          !scope.subjectIds.includes(ctx.subjectId)) ||
        ((scope.consumer.kind === 'worker' ||
          scope.consumer.kind === 'subject') &&
          !ctx.subjectId &&
          ![
            'delegations.create',
            'delegations.read',
            'delegations.update',
            'delegations.revoke',
            'delegations.select',
          ].includes(operation))
      )
        deny();
    } else if (!policy.allowedPlanes.includes('system')) deny();
    if (policy.targetRequired && !target) deny();
    const allowedTargets = scope
      ? targetBound(scope, operation, options.operationPolicy)
      : null;
    if (allowedTargets !== null && (!target || allowedTargets.length === 0))
      deny();
    if (target) {
      const refs =
        'product' in target
          ? [target]
          : target.kind === 'object'
            ? target.refs
            : target.kind === 'workspace'
              ? [target.workspaceRef]
              : [];
      if (
        refs.some(
          (ref) =>
            ref.installationId !== options.installationId ||
            ref.product !== options.product ||
            (scope &&
              ref.subjectId !== undefined &&
              !(scope.plane === 'system' && scope.subjectIds.length === 0) &&
              !scope.subjectIds.includes(ref.subjectId)) ||
            (ctx.subjectId !== undefined &&
              ref.subjectId !== undefined &&
              ref.subjectId !== ctx.subjectId),
        )
      )
        deny();
      if (allowedTargets !== null) {
        const actual: TargetScope =
          'product' in target
            ? { kind: 'object', refs: [target], actions: [operation] }
            : { ...target, actions: [operation] };
        if (!allowedTargets.some((parent) => targetContains(parent, actual)))
          deny();
      }
    }
    await options.sourceGuard?.(ctx, operation, target, database);
  }
  async function managed(
    ctx: CallContext,
    id: string,
    operation: string,
    database?: Kysely<any>,
  ): Promise<ServiceDelegation> {
    await authorize(
      ctx,
      operation,
      {
        kind: 'resource',
        connectorId: `${options.product}:${options.installationId}:credentials`,
        resourceIds: [id],
        actions: [operation],
      },
      database,
    );
    const stored = await get(id, database);
    if (!stored)
      throw problem(
        'credentials.delegation_not_found',
        'not_found',
        '委派不存在',
      );
    const { submissionKey: _receipt, ...value } = stored;
    const scope = await authority(ctx, database);
    if (!scope) return value;
    if (scope.plane === 'system' && scope.consumer.kind === 'maintenance') {
      if (
        scope.subjectIds.length === 0 ||
        (value.scope.subjectIds.length > 0 &&
          subset(value.scope.subjectIds, scope.subjectIds))
      )
        return value;
      deny();
    }
    const mine = ctx.actor.authorityRef;
    if (mine === `delegation:${value.id}`) return value;
    let parent = value.parentAuthorityRef;
    const seen = new Set<string>();
    while (
      parent.startsWith('delegation:') &&
      !seen.has(parent) &&
      seen.size < 32
    ) {
      if (parent === mine) return value;
      seen.add(parent);
      const row = await get(parent.slice(11), database);
      if (!row) break;
      parent = row.parentAuthorityRef;
    }
    deny();
  }
  async function issue(
    parent: string,
    userId: string,
    scope: DelegationScope,
    name: string,
    principalKind: Actor['kind'],
    issuer = parent,
    database?: Kysely<any>,
  ) {
    return transaction(database, async (tx) => {
      const id = uuid();
      await sql`insert into service_delegations(id,owner_user_id,parent_authority_ref,issuer_authority_ref,principal_kind,scope,revision,state) values(${id},${userId},${parent},${issuer},${principalKind},${JSON.stringify(scope)}::jsonb,1,'pending')`.execute(
        tx,
      );
      const key = await options.createKey(
        userId,
        {
          name,
          expiresIn: scope.expiresAt
            ? Math.max(
                1,
                Math.floor(
                  (Date.parse(scope.expiresAt.at) - Date.now()) / 1000,
                ),
              )
            : null,
        },
        tx,
      );
      await sql`update service_delegations set api_key_id=${key.id},state='active' where id=${id}`.execute(
        tx,
      );
      return {
        id: key.id,
        key: key.key,
        authorityRef: `delegation:${id}`,
        delegationId: id,
      };
    });
  }

  async function prepareIssue(
    ctx: CallContext,
    input: {
      name: string;
      scope: DelegationScope;
      principalKind: Actor['kind'];
    },
    database?: Kysely<any>,
  ) {
    await authorize(ctx, 'delegations.create', undefined, database);
    const scope = checkedScope(input.scope);
    if (!scope.expiresAt) deny();
    const parent = await authority(ctx, database);
    if (
      parent &&
      (!parent.mayDelegate ||
        !targetsContain(parent, scope, options.operationPolicy) ||
        !expiresWithin(scope, parent) ||
        (parent.notBefore &&
          (!scope.notBefore || scope.notBefore.at < parent.notBefore.at)))
    )
      deny();
    if (
      !(
        parent &&
        scopeContains(parent, scope, options.operationPolicy) &&
        (ctx.actor.kind === 'human' || ctx.actor.kind === input.principalKind)
      ) &&
      !(await options.issuancePolicy?.(
        {
          issuer: {
            actor: ctx.actor,
            consumer: ctx.consumer,
            scope: parent ?? undefined,
          },
          requested: scope,
          requestedPrincipalKind: input.principalKind,
        },
        database,
      ))
    )
      deny();
    if (ctx.actor.authorityRef.startsWith('session:')) {
      const session = await options.currentSession(
        ctx.actor.authorityRef.slice(8),
        database,
      );
      if (
        !session ||
        !scope.expiresAt ||
        scope.expiresAt.at > session.expiresAt
      )
        deny();
    }
    const userId = ctx.actor.authorityRef.startsWith('delegation:')
      ? (await active(ctx.actor.authorityRef.slice(11), database)).ownerUserId
      : ctx.actor.id;
    await guardSubjects(ctx, scope, database);
    return { scope, userId };
  }
  async function guardSubjects(
    ctx: CallContext,
    scope: DelegationScope,
    database?: Kysely<any>,
  ) {
    for (const subjectId of [...new Set(scope.subjectIds)].sort())
      await options.sourceGuard?.(
        { ...ctx, subjectId },
        'delegations.create',
        {
          product: options.product,
          installationId: options.installationId,
          subjectId,
          kind: 'subject',
          id: subjectId,
        },
        database,
      );
  }
  return {
    authorize,
    async currentAuthority(ctx: CallContext, database?: Kysely<any>) {
      const scope = await authority(ctx, database);
      const session = ctx.actor.authorityRef.startsWith('session:')
        ? await options.currentSession(
            ctx.actor.authorityRef.slice(8),
            database,
          )
        : null;
      return {
        scope,
        expiresAt:
          scope?.expiresAt ??
          (session ? { domainId: 'real', at: session.expiresAt } : undefined),
      };
    },
    async authenticateKey(apiKeyId: string) {
      const row = await sql<{
        id: string;
      }>`select id from service_delegations where api_key_id=${apiKeyId}`.execute(
        db,
      );
      if (!row.rows[0]) deny();
      const value = await active(row.rows[0].id);
      return {
        actor: {
          id: principalId(value),
          kind: value.principalKind,
          authorityRef: `delegation:${value.id}`,
        } satisfies Actor,
        consumer: value.scope.consumer,
      };
    },
    async createManagementKey(
      userId: string,
      input: { name: string; scope: DelegationScope },
    ) {
      const user = await options.currentUser(userId);
      const scope = checkedScope(input.scope);
      if (
        !user?.admin ||
        !user.enabled ||
        !scope.expiresAt ||
        scope.plane !== 'system' ||
        scope.subjectIds.length !== 0 ||
        !equal(scope.consumer, {
          kind: 'maintenance',
          host: options.product,
          id: options.installationId,
        })
      )
        deny();
      return issue(`user:${userId}`, userId, scope, input.name, 'human');
    },
    /** Product Subject lifecycle calls this after an explicit persistent runtime grant. */
    async createRuntimeKey(
      ctx: CallContext,
      input: { name: string; scope: DelegationScope },
    ) {
      return transaction(undefined, async (tx) => {
        await authorize(ctx, 'installation.manage', undefined, tx);
        const issuer = await authority(ctx, tx),
          scope = checkedScope(input.scope);
        if (
          ctx.actor.kind !== 'human' ||
          scope.plane !== 'subject' ||
          scope.consumer.kind !== 'subject' ||
          scope.consumer.host !== options.installationId ||
          scope.subjectIds.length !== 1 ||
          scope.subjectIds[0] !== scope.consumer.id ||
          scope.expiresAt !== null ||
          !(await options.issuancePolicy?.(
            {
              issuer: {
                actor: ctx.actor,
                consumer: ctx.consumer,
                scope: issuer ?? undefined,
              },
              requested: scope,
              requestedPrincipalKind: 'subject',
            },
            tx,
          ))
        )
          deny();
        const userId = ctx.actor.authorityRef.startsWith('delegation:')
          ? (await active(ctx.actor.authorityRef.slice(11), tx)).ownerUserId
          : ctx.actor.id;
        await guardSubjects(ctx, scope, tx);
        return issue(
          `user:${userId}`,
          userId,
          scope,
          input.name,
          'subject',
          ctx.actor.authorityRef,
          tx,
        );
      });
    },
    /** Data owner composition only; never restores an old grant on Subject import. */
    async deleteSubject(
      ctx: CallContext,
      input: { subjectId: string },
      database?: Kysely<any>,
    ) {
      const subjectId = SubjectIdSchema.parse(input.subjectId);
      if (ctx.subjectId && ctx.subjectId !== subjectId) deny();
      return transaction(database, async (tx) => {
        await authorize(
          { ...ctx, subjectId },
          'data.purge',
          {
            product: options.product,
            installationId: options.installationId,
            subjectId,
            kind: 'subject',
            id: subjectId,
          },
          tx,
        );
        const rows =
          await sql<ServiceDelegation>`select id,api_key_id,owner_user_id,parent_authority_ref,issuer_authority_ref,principal_kind,scope,revision,state from service_delegations where state in ('pending','active') and scope->'subjectIds' @> ${JSON.stringify([subjectId])}::jsonb order by id for update`.execute(
            tx,
          );
        let revoked = 0,
          narrowed = 0;
        for (const current of rows.rows) {
          const previous = DelegationScopeSchema.parse(current.scope);
          const subjectIds = previous.subjectIds.filter(
            (value) => value !== subjectId,
          );
          const targets = previous.targets.flatMap((target): TargetScope[] => {
            if (target.kind === 'workspace')
              return target.workspaceRef.subjectId === subjectId
                ? []
                : [target];
            if (target.kind !== 'object') return [target];
            const refs = target.refs.filter(
              (ref) => ref.subjectId !== subjectId,
            );
            return refs.length ? [{ ...target, refs }] : [];
          });
          // Removing the last positive target must not turn that operation into an unrestricted grant.
          const operations = previous.operations.filter(
            (operation) =>
              !previous.targets.some((target) =>
                target.actions.includes(operation),
              ) || targets.some((target) => target.actions.includes(operation)),
          );
          if (
            !subjectIds.length ||
            !operations.length ||
            previous.taskRef?.subjectId === subjectId ||
            previous.nativeBinding ||
            (previous.consumer.kind === 'subject' &&
              previous.consumer.id === subjectId)
          ) {
            await sql`update service_delegations set state='revoked',revision=revision+1,revoked_at=clock_timestamp() where id=${current.id}`.execute(
              tx,
            );
            if (current.apiKeyId)
              await options.disableKey(current.apiKeyId, tx);
            revoked++;
          } else {
            const scope = DelegationScopeSchema.parse({
              ...previous,
              subjectIds,
              operations,
              targets: targets
                .map((target) => ({
                  ...target,
                  actions: target.actions.filter((operation) =>
                    operations.includes(operation),
                  ),
                }))
                .filter((target) => target.actions.length),
              inputRefs: previous.inputRefs.filter(
                (ref) => ref.subjectId !== subjectId,
              ),
            });
            await sql`update service_delegations set scope=${JSON.stringify(scope)}::jsonb,revision=revision+1 where id=${current.id}`.execute(
              tx,
            );
            narrowed++;
          }
        }
        return { revoked, narrowed };
      });
    },
    async create(
      ctx: CallContext,
      input: {
        name: string;
        scope: DelegationScope;
        principalKind: Actor['kind'];
      },
      database?: Kysely<any>,
    ) {
      return transaction(database, async (tx) => {
        const { scope, userId } = await prepareIssue(ctx, input, tx);
        return issue(
          ctx.actor.authorityRef,
          userId,
          scope,
          input.name,
          input.principalKind,
          ctx.actor.authorityRef,
          tx,
        );
      });
    },
    async createChild(
      ctx: CallContext,
      input: {
        name: string;
        scope: DelegationScope;
        principalKind: Actor['kind'];
      },
    ) {
      if (
        ctx.actor.kind !== 'service' ||
        !ctx.actor.authorityRef.startsWith('delegation:')
      )
        deny();
      const submissionKey = createHash('sha256')
        .update(
          canonicalize([
            options.product,
            options.installationId,
            ctx.consumer,
            ctx.subjectId ?? null,
            null,
            ctx.trace.operationId,
            'delegations.createChild',
          ]),
        )
        .digest('hex');
      const requestDigest = createHash('sha256')
        .update(
          canonicalize({
            parent: ctx.actor.authorityRef,
            name: input.name,
            scope: input.scope,
            principalKind: input.principalKind,
          }),
        )
        .digest('hex');
      return db.transaction().execute(async (tx) => {
        await sql`select pg_advisory_xact_lock(hashtextextended(${submissionKey},0))`.execute(
          tx,
        );
        const { scope, userId } = await prepareIssue(ctx, input, tx);
        const issuer = await authority(ctx, tx);
        if (
          !issuer?.mayDelegate ||
          !issuer.operations.includes('delegations.select')
        )
          deny();
        const prior = await sql<
          ServiceDelegation & { requestDigest: string }
        >`select id,api_key_id,owner_user_id,parent_authority_ref,issuer_authority_ref,principal_kind,scope,revision,state,request_digest from service_delegations where submission_key=${submissionKey}`.execute(
          tx,
        );
        if (prior.rows[0]) {
          if (prior.rows[0].requestDigest !== requestDigest)
            throw problem(
              'credentials.submission_conflict',
              'conflict',
              '同一子委派提交包含不同授权',
            );
          const value = await active(prior.rows[0].id, tx);
          return {
            delegationId: value.id,
            authorityRef: `delegation:${value.id}`,
            revision: value.revision,
            scope: value.scope,
          };
        }
        const id = uuid();
        await sql`insert into service_delegations(id,owner_user_id,parent_authority_ref,issuer_authority_ref,principal_kind,scope,revision,state,submission_key,request_digest) values(${id},${userId},${ctx.actor.authorityRef},${ctx.actor.authorityRef},${input.principalKind},${JSON.stringify(scope)}::jsonb,1,'pending',${submissionKey},${requestDigest})`.execute(
          tx,
        );
        const key = await options.createKey(
          userId,
          {
            name: input.name,
            expiresIn: scope.expiresAt
              ? Math.max(
                  1,
                  Math.floor(
                    (Date.parse(scope.expiresAt.at) - Date.now()) / 1000,
                  ),
                )
              : null,
          },
          tx,
        );
        await sql`update service_delegations set api_key_id=${key.id},state='active' where id=${id}`.execute(
          tx,
        );
        return {
          delegationId: id,
          authorityRef: `delegation:${id}`,
          revision: 1,
          scope,
        };
      });
    },
    async selectDelegation(
      ctx: CallContext,
      authorityRef: string,
    ): Promise<CallContext> {
      if (
        ctx.actor.kind !== 'service' ||
        !ctx.actor.authorityRef.startsWith('delegation:') ||
        !/^delegation:[0-9a-f-]{36}$/.test(authorityRef)
      )
        deny();
      const child = await active(authorityRef.slice(11));
      if (child.parentAuthorityRef !== ctx.actor.authorityRef) deny();
      const parent = await active(ctx.actor.authorityRef.slice(11));
      if (
        !parent.scope.mayDelegate ||
        !parent.scope.operations.includes('delegations.select')
      )
        deny();
      const subjectId =
        ctx.subjectId ??
        (child.scope.subjectIds.length === 1
          ? child.scope.subjectIds[0]
          : undefined);
      await authorize({ ...ctx, subjectId }, 'delegations.select', {
        kind: 'resource',
        connectorId: `${options.product}:${options.installationId}:credentials`,
        resourceIds: [child.id],
        actions: ['delegations.select'],
      });
      return {
        ...ctx,
        subjectId,
        actor: {
          kind: child.principalKind,
          id: principalId(child),
          authorityRef,
        },
        consumer: child.scope.consumer,
      };
    },
    async read(ctx: CallContext, id: string) {
      const value = await managed(ctx, id, 'delegations.read');
      return value;
    },
    async update(
      ctx: CallContext,
      id: string,
      input: { expectedRevision: number; scope: DelegationScope },
    ) {
      const prior = await managed(ctx, id, 'delegations.update');
      const scope = checkedScope(input.scope);
      if (!scopeContains(prior.scope, scope, options.operationPolicy)) deny();
      const result =
        await sql`update service_delegations set scope=${JSON.stringify(scope)}::jsonb,revision=revision+1 where id=${id} and revision=${input.expectedRevision} and state='active'`.execute(
          db,
        );
      if (result.numAffectedRows !== 1n)
        throw problem(
          'credentials.revision_conflict',
          'conflict',
          '委派版本已变化',
        );
      return get(id);
    },
    async revoke(
      ctx: CallContext,
      id: string,
      input: { expectedRevision: number },
      database?: Kysely<any>,
    ) {
      return transaction(database, async (tx) => {
        const prior = await managed(ctx, id, 'delegations.revoke', tx);
        const result =
          await sql`update service_delegations set state='revoked',revision=revision+1,revoked_at=clock_timestamp() where id=${id} and revision=${input.expectedRevision}`.execute(
            tx,
          );
        if (result.numAffectedRows !== 1n)
          throw problem(
            'credentials.revision_conflict',
            'conflict',
            '委派版本已变化',
          );
        if (prior.apiKeyId) await options.disableKey(prior.apiKeyId, tx);
        return { id, revoked: true };
      });
    },
  };
}
