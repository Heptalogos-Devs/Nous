import { requireSubject } from '../platform/security.js';
import { parseInstant } from '@nous-wave/platform/time';
import { sql } from 'kysely';
import {
  canonicalRefKey,
  type CallContext,
  type Policy,
  type Ref,
  type Selector,
  type TimePoint,
} from '@nous-wave/contracts/common';
import type { SourceResolution } from '@nous-wave/contracts/materials';
import { DomainError, fail } from '../platform/errors.js';
import {
  intersectPolicy,
  mayRead,
  mayReadScope,
  isAccessDenied,
} from '../platform/security.js';
import { reference, type Storage } from '../platform/context.js';
import {
  normalizedSelector,
  type SourceSnapshot,
  type createSourceRepository,
} from '../materials/source-repository.js';
import type {
  KnowledgeRecord,
  createKnowledgeRepository,
} from './repository.js';

const identity = (ref: Ref) => `${ref.kind}:${ref.id}:${ref.revision ?? 0}`;
type Node = SourceSnapshot & { formed?: TimePoint; formation?: string };
export interface SourceCutoff {
  knowledgeAt?: TimePoint;
  recordedAsOf?: string;
}
export interface ResolvedSource extends SourceResolution {
  known: boolean;
}
export interface PreparedSourcePolicies {
  entries: Array<{ ref: Ref; policy: Policy }>;
}
export interface SourceReadOptions {
  db: Storage['db'];
  external: PreparedSourcePolicies;
}

function regionContains(
  outer: Selector | undefined,
  inner: Selector | undefined,
  length?: number,
): boolean {
  if (!outer) return true;
  if (!inner)
    return outer.kind === 'text'
      ? (outer.start ?? 0) === 0 && outer.end === length
      : outer.kind === 'image'
        ? (outer.x ?? 0) === 0 &&
          (outer.y ?? 0) === 0 &&
          (outer.width ?? 1) === 1 &&
          (outer.height ?? 1) === 1
        : false;
  if (outer.kind !== inner.kind || !regionContains(outer.within, inner.within))
    return false;
  if (outer.kind === 'node')
    return inner.kind === 'node' && outer.pointer === inner.pointer;
  if (inner.kind === 'node') return false;
  if (outer.kind === 'text' || outer.kind === 'media')
    return (
      (outer.start ?? 0) <= (inner.start ?? 0) &&
      (outer.end ?? Infinity) >= (inner.end ?? length ?? Infinity)
    );
  if (outer.kind === 'image')
    return (
      (outer.x ?? 0) <= (inner.x ?? 0) &&
      (outer.y ?? 0) <= (inner.y ?? 0) &&
      (outer.x ?? 0) + (outer.width ?? 1) >=
        (inner.x ?? 0) + (inner.width ?? 1) &&
      (outer.y ?? 0) + (outer.height ?? 1) >=
        (inner.y ?? 0) + (inner.height ?? 1)
    );
  return outer.kind === 'page'
    ? outer.page === inner.page
    : outer.partId === inner.partId;
}
export function observedRegion(
  requested: Ref,
  observations: Ref[],
  length?: number,
) {
  const matching = observations.filter(
    (ref) =>
      ref.id === requested.id &&
      ref.kind === requested.kind &&
      (ref.revision === requested.revision ||
        (ref.revision === undefined && requested.revision === 1)),
  );
  if (
    matching.some((ref) =>
      regionContains(ref.selector, requested.selector, length),
    )
  )
    return true;
  if (
    requested.selector?.kind !== 'text' &&
    !(requested.selector === undefined && length !== undefined)
  )
    return false;
  const start = requested.selector?.start ?? 0,
    end = requested.selector?.end ?? length!;
  let covered = start;
  for (const interval of matching
    .flatMap((ref) =>
      ref.selector?.kind === 'text' && !ref.selector.within
        ? [[ref.selector.start ?? 0, ref.selector.end ?? length ?? 0]]
        : [],
    )
    .sort((a, b) => a[0]! - b[0]!)) {
    if (interval[0]! > covered) break;
    covered = Math.max(covered, interval[1]!);
    if (covered >= end) return true;
  }
  return false;
}

export function createSourceResolver(
  store: Storage & {
    sourcePolicies?: (ctx: CallContext, refs: Ref[]) => Promise<Policy[]>;
  },
  knowledge: ReturnType<typeof createKnowledgeRepository>,
  material: ReturnType<typeof createSourceRepository>,
) {
  async function externalPolicies(ctx: CallContext, refs: Ref[]) {
    requireSubject(ctx);
    const external = [
      ...new Map(
        refs
          .filter(
            (ref) =>
              ref.product !== 'nous' ||
              ref.installationId !== store.installationId,
          )
          .map((ref) => [canonicalRefKey(ref), ref]),
      ).values(),
    ];
    if (!external.length) return new Map<string, Policy>();
    if (!store.sourcePolicies)
      fail('capability_unavailable', '外部来源的当前授权提供者未配置');
    const result = new Map<string, Policy>();
    for (let offset = 0; offset < external.length; offset += 200) {
      const batch = external.slice(offset, offset + 200),
        policies = await store.sourcePolicies(ctx, batch);
      if (policies.length !== batch.length)
        fail('dependency_unavailable', '外部来源授权结果没有逐项对应请求');
      for (const [index, ref] of batch.entries())
        result.set(canonicalRefKey(ref), policies[index]!);
    }
    return result;
  }
  function knowledgeNode(record: KnowledgeRecord): Node {
    const { object, revision } = record;
    return {
      requested: reference(
        store.installationId,
        object.subjectId,
        object.kind,
        object.id,
        revision.revision,
      ),
      ref: reference(
        store.installationId,
        object.subjectId,
        object.kind,
        object.id,
        revision.revision,
      ),
      inputs: revision.sourceRefs,
      roots: revision.sourceRoots,
      policy: object.policy,
      recorded: { domainId: 'real', at: parseInstant(revision.recordedAt) },
      formed: revision.formed,
      formation: revision.formation,
      textLength: Array.from(revision.body.content).length,
      mediaType: 'text/plain',
    };
  }
  async function resolveManyDetailed(
    ctx: CallContext,
    refs: Ref[],
    cutoff: SourceCutoff = {},
    supplied: KnowledgeRecord[] = [],
    enforceScope = true,
    read?: SourceReadOptions,
  ) {
    requireSubject(ctx);
    const db = read?.db ?? store.db;
    const nodes = new Map<string, Node>();
    supplied.forEach((record) => {
      const node = knowledgeNode(record);
      nodes.set(identity(node.ref), node);
    });
    const normalized = new Map<string, Ref>();
    const errors = new Map<string, DomainError>();
    async function load(requests: Ref[]) {
      const missing = [
        ...new Map(
          requests
            .filter((ref) => !nodes.has(identity(ref)))
            .map((ref) => [canonicalRefKey(ref), ref]),
        ).values(),
      ];
      const metadata = missing.filter((ref) =>
        ['Material', 'Encounter', 'ExperienceItem'].includes(ref.kind),
      );
      const mental = missing.filter(
        (ref) =>
          !['Material', 'Encounter', 'ExperienceItem'].includes(ref.kind),
      );
      const fetched = await material.snapshots(ctx, metadata, db);
      const records = await knowledge.records(
        ctx,
        mental.map((ref) => ({ id: ref.id, revision: ref.revision })),
        db,
      );
      const candidates = [
        ...fetched.map((node) => ({ request: node.requested, node })),
        ...records.map((record) => ({
          request: mental.find(
            (ref) =>
              ref.id === record.requested.id &&
              ref.revision === record.requested.revision,
          )!,
          node: knowledgeNode(record),
        })),
      ];
      for (const { request, node } of candidates) {
        if (!request) continue;
        nodes.set(identity(node.ref), node);
        const ref = {
          ...node.ref,
          selector: normalizedSelector(
            request.selector,
            node.textLength,
            node.mediaType,
          ),
        };
        normalized.set(canonicalRefKey(request), ref);
      }
      for (const ref of requests) {
        if (!normalized.has(canonicalRefKey(ref))) {
          const node = nodes.get(identity(ref));
          if (node)
            normalized.set(canonicalRefKey(ref), {
              ...node.ref,
              selector: normalizedSelector(
                ref.selector,
                node.textLength,
                node.mediaType,
              ),
            });
          else
            errors.set(
              canonicalRefKey(ref),
              new DomainError('not_found', '来源或所选修订不存在'),
            );
        }
      }
    }
    for (const ref of refs)
      if (
        ref.product !== 'nous' ||
        ref.installationId !== store.installationId ||
        ref.subjectId !== ctx.subjectId
      )
        fail('invalid_input', '来源必须先经本主体正式材料或经历入口接纳');
    await load(refs);
    if (enforceScope)
      for (const requested of refs) {
        const key = canonicalRefKey(requested),
          ref = normalized.get(key);
        if (!ref) continue;
        try {
          await store.authorize(
            ctx,
            ref.kind === 'Material'
              ? 'materials.read'
              : ['Encounter', 'ExperienceItem'].includes(ref.kind)
                ? 'encounters.read'
                : 'knowledge.read',
            ref,
            db,
          );
          const node = nodes.get(identity(ref));
          if (node?.scope && !mayReadScope(ctx, node.scope))
            await store.authorize(ctx, 'data.manage', ref, db);
        } catch (error) {
          if (isAccessDenied(error))
            errors.set(
              key,
              new DomainError('forbidden', '来源不在本次当前授权内'),
            );
          else throw error;
        }
      }
    const starts = refs
      .filter((ref) => !errors.has(canonicalRefKey(ref)))
      .flatMap((ref) => normalized.get(canonicalRefKey(ref)) ?? []);
    const reachable = starts.length
      ? (
          await sql<{ ref: Ref }>`with recursive reachable(ref) as(
      select value from jsonb_array_elements(${JSON.stringify(starts)}::jsonb)
      union select l.to_ref from reachable r join source_links l on l.subject_id=${ctx.subjectId}::uuid
        and l.from_ref->>'kind'=r.ref->>'kind' and l.from_ref->>'id'=r.ref->>'id'
        and coalesce(l.from_ref->>'revision','0')=coalesce(r.ref->>'revision','0')
    ) select ref from reachable`.execute(db)
        ).rows.map((row) => row.ref)
      : [];
    await load(reachable);
    const observations = cutoff.knowledgeAt
      ? await material.observations(
          ctx,
          [...new Set([...nodes.values()].map((node) => node.ref.id))],
          cutoff.knowledgeAt,
          cutoff.recordedAsOf,
          db,
        )
      : [];
    const memo = new Map<
      string,
      { policy: Policy; roots: Ref[]; known: boolean }
    >();
    function effective(
      ref: Ref,
      stack = new Set<string>(),
    ): { policy: Policy; roots: Ref[]; known: boolean } {
      const key = canonicalRefKey(ref),
        cached = memo.get(key);
      if (cached) return cached;
      if (stack.has(key)) fail('invalid_input', '来源包含循环依赖');
      const node = nodes.get(identity(ref));
      if (!node)
        throw (
          errors.get(key) ??
          new DomainError(
            'dependency_unavailable',
            '来源导航缺少正式输入，需要重建',
          )
        );
      if (!mayRead(ctx, node.policy))
        fail('forbidden', '依赖来源已限制本次用途或听众');
      stack.add(key);
      const dependencies = node.inputs.map((input) =>
        effective(normalized.get(canonicalRefKey(input)) ?? input, stack),
      );
      stack.delete(key);
      const policy = intersectPolicy([
        node.policy,
        ...dependencies.map((dependency) => dependency.policy),
      ]);
      if (!mayRead(ctx, policy))
        fail('forbidden', '依赖来源已限制本次用途或听众');
      const roots = [
        ...new Map(
          (dependencies.length
            ? dependencies.flatMap((dependency) => dependency.roots)
            : node.roots.length
              ? node.roots
              : ['Encounter', 'ExperienceItem'].includes(node.ref.kind)
                ? [node.ref]
                : []
          ).map((root) => [canonicalRefKey(root), root]),
        ).values(),
      ];
      let known = true;
      if (cutoff.knowledgeAt) {
        const at = cutoff.knowledgeAt;
        const observed = observedRegion(
          ref,
          observations.map((item) => item.ref),
          node.textLength,
        );
        if (node.formed)
          known =
            node.formed.domainId === at.domainId &&
            node.formed.at <= at.at &&
            (['operator_edit', 'initial_setting'].includes(
              node.formation ?? '',
            ) ||
              observed ||
              dependencies.every((dependency) => dependency.known));
        else if (node.observed)
          known =
            node.consumerKind === 'subject' &&
            node.observed.domainId === at.domainId &&
            node.observed.at <= at.at;
        else known = observed;
        if (cutoff.recordedAsOf && node.recorded.at > cutoff.recordedAsOf)
          known = false;
      }
      const result = { policy, roots, known };
      memo.set(key, result);
      return result;
    }
    const results = new Map<string, ResolvedSource | DomainError>();
    for (const requested of refs) {
      const key = canonicalRefKey(requested),
        ref = normalized.get(key);
      if (errors.has(key)) {
        results.set(key, errors.get(key)!);
        continue;
      }
      if (!ref) {
        results.set(key, new DomainError('not_found', '来源不存在'));
        continue;
      }
      try {
        const result = effective(ref);
        results.set(key, {
          resolvedRefs: [ref],
          roots: result.roots,
          policies: [result.policy],
          known: result.known,
        });
      } catch (error) {
        if (isAccessDenied(error))
          results.set(
            key,
            new DomainError('forbidden', '来源不在本次当前授权内'),
          );
        else if (error instanceof DomainError) results.set(key, error);
        else throw error;
      }
    }
    const externalRefs = [
      ...new Map(
        [...results.values()]
          .flatMap((result) =>
            result instanceof DomainError ? [] : result.roots,
          )
          .filter(
            (ref) =>
              ref.product !== 'nous' ||
              ref.installationId !== store.installationId,
          )
          .map((ref) => [canonicalRefKey(ref), ref]),
      ).values(),
    ];
    const external = read
      ? new Map(
          read.external.entries.map((entry) => [
            canonicalRefKey(entry.ref),
            entry.policy,
          ]),
        )
      : await externalPolicies(ctx, externalRefs);
    if (externalRefs.some((ref) => !external.has(canonicalRefKey(ref))))
      fail('conflict', '采用组包含尚未准备的外部来源');
    for (const [key, result] of results) {
      if (result instanceof DomainError) continue;
      const policies = result.roots.flatMap(
        (ref) => external.get(canonicalRefKey(ref)) ?? [],
      );
      const policy = intersectPolicy([...result.policies, ...policies]);
      results.set(
        key,
        mayRead(ctx, policy)
          ? { ...result, policies: [policy] }
          : new DomainError('forbidden', '外部来源已限制本次用途或听众'),
      );
    }
    return {
      results,
      external: externalRefs.map((ref) => ({
        ref,
        policy: external.get(canonicalRefKey(ref))!,
      })),
    };
  }
  async function resolveMany(
    ctx: CallContext,
    refs: Ref[],
    cutoff: SourceCutoff = {},
    supplied: KnowledgeRecord[] = [],
    enforceScope = true,
    read?: SourceReadOptions,
  ) {
    requireSubject(ctx);
    return (
      await resolveManyDetailed(ctx, refs, cutoff, supplied, enforceScope, read)
    ).results;
  }
  async function prepare(
    ctx: CallContext,
    refs: Ref[],
  ): Promise<PreparedSourcePolicies> {
    requireSubject(ctx);
    const entries: PreparedSourcePolicies['entries'] = [];
    for (let offset = 0; offset < refs.length; offset += 200) {
      const result = await resolveManyDetailed(
        ctx,
        refs.slice(offset, offset + 200),
      );
      for (const resolved of result.results.values())
        if (resolved instanceof DomainError) throw resolved;
      entries.push(...result.external);
    }
    return { entries };
  }
  async function resolve(
    ctx: CallContext,
    refs: Ref[],
    cutoff: SourceCutoff = {},
    enforceScope = true,
    read?: SourceReadOptions,
  ): Promise<ResolvedSource> {
    requireSubject(ctx);
    const results = await resolveMany(
      ctx,
      refs,
      cutoff,
      [],
      enforceScope,
      read,
    );
    const resolvedRefs: Ref[] = [],
      roots = new Map<string, Ref>(),
      policies: Policy[] = [];
    let known = true;
    for (const ref of refs) {
      const result = results.get(canonicalRefKey(ref))!;
      if (result instanceof DomainError) throw result;
      resolvedRefs.push(...result.resolvedRefs);
      result.roots.forEach((root) => roots.set(canonicalRefKey(root), root));
      policies.push(...result.policies);
      known &&= result.known;
    }
    return {
      resolvedRefs: [
        ...new Map(
          resolvedRefs.map((ref) => [canonicalRefKey(ref), ref]),
        ).values(),
      ],
      roots: [...roots.values()],
      policies,
      known,
    };
  }
  return {
    resolve,
    resolveMany,
    prepare,
    externalPolicies: async (ctx: CallContext, refs: Ref[]) => [
      ...(await externalPolicies(ctx, refs)).values(),
    ],
    async readable(ctx: CallContext, refs: Ref[]) {
      requireSubject(ctx);
      const results = await resolveMany(ctx, refs);
      return refs.flatMap((ref) => {
        const result = results.get(canonicalRefKey(ref))!;
        if (result instanceof DomainError) {
          if (['not_found', 'forbidden'].includes(result.code)) return [];
          throw result;
        }
        return result.resolvedRefs;
      });
    },
    inherit: (ctx: CallContext, refs: Ref[]) => resolve(ctx, refs, {}, false),
  };
}
