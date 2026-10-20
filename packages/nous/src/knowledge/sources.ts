import { sql } from 'kysely';
import { canonicalRefKey, type CallContext, type Policy, type Ref, type Selector, type TimePoint } from '@nous-wave/contracts/common';
import type { SourceResolution } from '@nous-wave/contracts/materials';
import { DomainError, fail } from '../platform/errors.js';
import { authorize, intersectPolicy, mayRead,mayReadScope } from '../platform/security.js';
import { reference, type Storage } from '../platform/context.js';
import { normalizedSelector, type SourceSnapshot, type createSourceRepository } from '../materials/source-repository.js';
import type { KnowledgeRecord, createKnowledgeRepository } from './repository.js';

const identity = (ref: Ref) => `${ref.kind}:${ref.id}:${ref.revision ?? 0}`;
type Node = SourceSnapshot & { formed?: TimePoint; formation?: string };
export interface SourceCutoff { knowledgeAt?: TimePoint; recordedAsOf?: string }
export interface ResolvedSource extends SourceResolution { known: boolean }

function regionContains(outer: Selector | undefined, inner: Selector | undefined, length?: number): boolean {
  if (!outer) return true;
  if (!inner) return outer.kind === 'text' ? (outer.start ?? 0) === 0 && outer.end === length : outer.kind === 'image' ? (outer.x ?? 0) === 0 && (outer.y ?? 0) === 0 && (outer.width ?? 1) === 1 && (outer.height ?? 1) === 1 : false;
  if (outer.kind !== inner.kind || !regionContains(outer.within, inner.within)) return false;
  if (outer.kind === 'text' || outer.kind === 'media') return (outer.start ?? 0) <= (inner.start ?? 0) && (outer.end ?? Infinity) >= (inner.end ?? length ?? Infinity);
  if (outer.kind === 'image') return (outer.x ?? 0) <= (inner.x ?? 0) && (outer.y ?? 0) <= (inner.y ?? 0) && (outer.x ?? 0) + (outer.width ?? 1) >= (inner.x ?? 0) + (inner.width ?? 1) && (outer.y ?? 0) + (outer.height ?? 1) >= (inner.y ?? 0) + (inner.height ?? 1);
  return outer.kind === 'page' ? outer.page === inner.page : outer.kind === 'part' ? outer.partId === inner.partId : outer.nodeId === inner.nodeId;
}
export function observedRegion(requested: Ref, observations: Ref[], length?: number) {
  const matching = observations.filter(ref => ref.id === requested.id && ref.kind === requested.kind && (ref.revision === requested.revision || ref.revision === undefined && requested.revision === 1));
  if (matching.some(ref => regionContains(ref.selector, requested.selector, length))) return true;
  if (requested.selector?.kind !== 'text' && !(requested.selector === undefined && length !== undefined)) return false;
  const start = requested.selector?.start ?? 0, end = requested.selector?.end ?? length!;
  let covered = start;
  for (const interval of matching.filter(ref => ref.selector?.kind === 'text' && !ref.selector.within).map(ref => [ref.selector!.start ?? 0, ref.selector!.end ?? length ?? 0]).sort((a, b) => a[0]! - b[0]!)) {
    if (interval[0]! > covered) break;
    covered = Math.max(covered, interval[1]!);
    if (covered >= end) return true;
  }
  return false;
}

export function createSourceResolver(store: Storage & {sourcePolicies?:(ctx:CallContext,refs:Ref[])=>Promise<Policy[]>}, knowledge: ReturnType<typeof createKnowledgeRepository>, material: ReturnType<typeof createSourceRepository>) {
  async function externalPolicies(ctx:CallContext,refs:Ref[]){
    const external=[...new Map(refs.filter(ref=>ref.product!=='nous'||ref.installationId!==store.installationId).map(ref=>[canonicalRefKey(ref),ref])).values()];
    if(!external.length)return new Map<string,Policy>();if(!store.sourcePolicies)fail('capability_unavailable','外部来源的当前授权提供者未配置');
    const result=new Map<string,Policy>();
    for(let offset=0;offset<external.length;offset+=200){const batch=external.slice(offset,offset+200),policies=await store.sourcePolicies(ctx,batch);if(policies.length!==batch.length)fail('dependency_unavailable','外部来源授权结果没有逐项对应请求');for(const[index,ref]of batch.entries())result.set(canonicalRefKey(ref),policies[index]!);}
    return result;
  }
  function knowledgeNode(record: KnowledgeRecord): Node {
    const { object, revision } = record;
    return { requested: reference(store.installationId, object.subjectId, object.kind, object.id, revision.revision), ref: reference(store.installationId, object.subjectId, object.kind, object.id, revision.revision),
      inputs: revision.sourceRefs, roots: revision.sourceRoots, policy: object.policy, recorded: { clockId: 'real', at: new Date(revision.recordedAt).toISOString() },
      formed: revision.formed, formation: revision.formation, textLength: Array.from(revision.body.content).length, mediaType: 'text/plain' };
  }
  async function resolveMany(ctx: CallContext, refs: Ref[], cutoff: SourceCutoff = {}, supplied: KnowledgeRecord[] = [], enforceScope=true) {
    const nodes = new Map<string, Node>();
    supplied.forEach(record => { const node = knowledgeNode(record); nodes.set(identity(node.ref), node); });
    const normalized = new Map<string, Ref>();
    const errors = new Map<string, DomainError>();
    async function load(requests: Ref[]) {
      const missing = [...new Map(requests.filter(ref => !nodes.has(identity(ref))).map(ref => [canonicalRefKey(ref), ref])).values()];
      const metadata = missing.filter(ref => ['Material', 'Encounter', 'ExperienceItem'].includes(ref.kind));
      const mental = missing.filter(ref => !['Material', 'Encounter', 'ExperienceItem'].includes(ref.kind));
      if(metadata.some(ref=>ref.kind==='Material'))authorize(ctx,'materials.read');
      if(mental.length)authorize(ctx,'knowledge.read');
      const fetched = await material.snapshots(ctx, metadata);
      const records = await knowledge.records(ctx, mental.map(ref => ({ id: ref.id, revision: ref.revision })));
      const candidates = [...fetched.map(node => ({ request: node.requested, node })), ...records.map(record => ({ request: mental.find(ref => ref.id === record.requested.id && ref.revision === record.requested.revision)!, node: knowledgeNode(record) }))];
      for (const { request, node } of candidates) {
        if (!request) continue;
        nodes.set(identity(node.ref), node);
        const ref = { ...node.ref, selector: normalizedSelector(request.selector, node.textLength, node.mediaType) };
        normalized.set(canonicalRefKey(request), ref);
      }
      for (const ref of requests) {
        if (!normalized.has(canonicalRefKey(ref))) {
          const node = nodes.get(identity(ref));
          if (node) normalized.set(canonicalRefKey(ref), { ...node.ref, selector: normalizedSelector(ref.selector, node.textLength, node.mediaType) });
          else errors.set(canonicalRefKey(ref), new DomainError('not_found', '来源或所选修订不存在'));
        }
      }
    }
    for (const ref of refs) if (ref.product !== 'nous' || ref.installationId !== store.installationId || ref.subjectId !== ctx.subjectId) fail('invalid_input', '来源必须先经本主体正式材料或经历入口接纳');
    await load(refs);
    const starts = refs.flatMap(ref => normalized.get(canonicalRefKey(ref)) ?? []);
    const reachable = starts.length ? (await sql<{ ref: Ref }>`with recursive reachable(ref) as(
      select value from jsonb_array_elements(${JSON.stringify(starts)}::jsonb)
      union select l.to_ref from reachable r join source_links l on l.subject_id=${ctx.subjectId}::uuid
        and l.from_ref->>'kind'=r.ref->>'kind' and l.from_ref->>'id'=r.ref->>'id'
        and coalesce(l.from_ref->>'revision','0')=coalesce(r.ref->>'revision','0')
    ) select ref from reachable`.execute(store.db)).rows.map(row => row.ref) : [];
    await load(reachable);
    const observations = cutoff.knowledgeAt ? await material.observations(ctx, [...new Set([...nodes.values()].map(node => node.ref.id))], cutoff.knowledgeAt, cutoff.recordedAsOf) : [];
    const memo = new Map<string, { policy: Policy; roots: Ref[]; known: boolean }>();
    function effective(ref: Ref, stack = new Set<string>()): { policy: Policy; roots: Ref[]; known: boolean } {
      const key = canonicalRefKey(ref), cached = memo.get(key);
      if (cached) return cached;
      if (stack.has(key)) fail('invalid_input', '来源包含循环依赖');
      const node = nodes.get(identity(ref));
      if (!node) throw errors.get(key) ?? new DomainError('dependency_unavailable', '来源导航缺少正式输入，需要重建');
      if (!mayRead(ctx, node.policy)) fail('forbidden', '依赖来源已限制本次用途或听众');
      stack.add(key);
      const dependencies = node.inputs.map(input => effective(normalized.get(canonicalRefKey(input)) ?? input, stack));
      stack.delete(key);
      const policy = intersectPolicy([node.policy, ...dependencies.map(dependency => dependency.policy)]);
      if (!mayRead(ctx, policy)) fail('forbidden', '依赖来源已限制本次用途或听众');
      const roots = [...new Map((dependencies.length ? dependencies.flatMap(dependency => dependency.roots) : node.roots.length ? node.roots : ['Encounter','ExperienceItem'].includes(node.ref.kind) ? [node.ref] : []).map(root => [canonicalRefKey(root), root])).values()];
      let known = true;
      if (cutoff.knowledgeAt) {
        const at = cutoff.knowledgeAt;
        const observed = observedRegion(ref, observations.map(item => item.ref), node.textLength);
        if (node.formed) known = node.formed.clockId === at.clockId && node.formed.at <= at.at && (['operator_edit','initial_setting'].includes(node.formation ?? '') || observed || dependencies.every(dependency => dependency.known));
        else if (node.observed) known = node.consumerKind === 'subject' && node.observed.clockId === at.clockId && node.observed.at <= at.at;
        else known = observed;
        if (cutoff.recordedAsOf && node.recorded.at > cutoff.recordedAsOf) known = false;
      }
      const result = { policy, roots, known }; memo.set(key, result); return result;
    }
    const results = new Map<string, ResolvedSource | DomainError>();
    for (const requested of refs) {
      const key = canonicalRefKey(requested), ref = normalized.get(key);
      if (!ref) { results.set(key, errors.get(key) ?? new DomainError('not_found', '来源不存在')); continue; }
      try { const node=nodes.get(identity(ref));if(enforceScope&&node?.scope&&!mayReadScope(ctx,node.scope))fail('forbidden','来源属于另一个执行范围');const result = effective(ref); results.set(key, { resolvedRefs: [ref], roots: result.roots, policies: [result.policy], known: result.known }); }
      catch (error) { if (!(error instanceof DomainError)) throw error; results.set(key, error); }
    }
    const external=await externalPolicies(ctx,[...results.values()].flatMap(result=>result instanceof DomainError?[]:result.roots));
    for(const[key,result]of results){if(result instanceof DomainError)continue;const policies=result.roots.flatMap(ref=>external.get(canonicalRefKey(ref))??[]);const policy=intersectPolicy([...result.policies,...policies]);results.set(key,mayRead(ctx,policy)?{...result,policies:[policy]}:new DomainError('forbidden','外部来源已限制本次用途或听众'));}
    return results;
  }
  async function resolve(ctx: CallContext, refs: Ref[], cutoff: SourceCutoff = {},enforceScope=true): Promise<ResolvedSource> {
    const results = await resolveMany(ctx, refs, cutoff,[],enforceScope);
    const resolvedRefs: Ref[] = [], roots = new Map<string, Ref>(), policies: Policy[] = [];
    let known = true;
    for (const ref of refs) { const result = results.get(canonicalRefKey(ref))!; if (result instanceof DomainError) throw result; resolvedRefs.push(...result.resolvedRefs); result.roots.forEach(root => roots.set(canonicalRefKey(root), root)); policies.push(...result.policies); known &&= result.known; }
    return { resolvedRefs: [...new Map(resolvedRefs.map(ref => [canonicalRefKey(ref), ref])).values()], roots: [...roots.values()], policies, known };
  }
  return {resolve,resolveMany,externalPolicies:async(ctx:CallContext,refs:Ref[])=>[...(await externalPolicies(ctx,refs)).values()],async readable(ctx:CallContext,refs:Ref[]){const results=await resolveMany(ctx,refs);return refs.flatMap(ref=>{const result=results.get(canonicalRefKey(ref))!;if(result instanceof DomainError){if(['not_found','forbidden'].includes(result.code))return [];throw result;}return result.resolvedRefs;});},inherit:(ctx:CallContext,refs:Ref[])=>resolve(ctx,refs,{},false)};
}
