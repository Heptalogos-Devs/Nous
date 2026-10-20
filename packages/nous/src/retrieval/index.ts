import { createHash } from 'node:crypto';
import * as lance from '@lancedb/lancedb';
import { MultiDirectedGraph } from 'graphology';
import type { z } from 'zod';
import { Matrix, SingularValueDecomposition } from 'ml-matrix';
import { v7 as uuid } from 'uuid';
import type { CallContext, Ref, TimePoint, TimeRange } from '@nous-wave/contracts/common';
import { canonicalRefKey } from '@nous-wave/contracts/common';
import { ExpandSchema, ExpansionResultSchema, QueryResultSchema, type Query, type QueryInput, type QueryResult } from '@nous-wave/contracts/cognition';
import type { KnowledgeKind, KnowledgeView,KnowledgePage,KnowledgeListSchema,KnowledgeReadSchema } from '@nous-wave/contracts/knowledge';
import type { MaterialView } from '@nous-wave/contracts/materials';
import { resolveQuery } from './nousql.js';
import { DomainError, fail } from '../platform/errors.js';

export { resolveQuery, QuerySyntaxError } from './nousql.js';

export interface RetrievalDependencies {
  installationId: string;
  indexPath: string;
  knowledge: {
    list(ctx: CallContext, input: z.input<typeof KnowledgeListSchema>): Promise<KnowledgePage>;
    readMany(ctx:CallContext,requests:z.input<typeof KnowledgeReadSchema>[]):Promise<KnowledgeView[]>;
    read(ctx: CallContext, input: { id: string; revision?: number; knowledgeAt?: Query['knowledgeAt']; recordedAsOf?: string }): Promise<KnowledgeView>;
  };
  embed?: { id: string; dimensions: number; embed(ctx: CallContext, texts: string[]): Promise<number[][]> };
  relations?(ctx: CallContext, refs: Ref[], limit: number,cutoff?:{knowledgeAt?:TimePoint;recordedAsOf?:string}): Promise<Array<{ from: Ref; to: Ref; relation: string; weight: number }>>;
  sourceTimes?(ctx: CallContext, refs: Ref[]): Promise<{ occurred: TimeRange[]; observed: TimePoint[] }>;
  materials?: { read(ctx: CallContext, input: { ref: Ref }): Promise<MaterialView> };
  encounterInputs?(ctx: CallContext, ref: Ref): Promise<Ref[]>;
  capabilities?(ctx: CallContext): Promise<KnowledgeKind[]>;
  authorize(ctx: CallContext, operation: string): void;
}

const keyOf = (view: KnowledgeView) => `${view.ref.id}:${view.revision}`;
const quoted = (value: string) => `'${value.replaceAll("'", "''")}'`;
const sourceKey = (ref: Ref) => `${ref.product}:${ref.installationId}:${ref.id}`;
const defaults: KnowledgeKind[] = ['Memory', 'Episode', 'Journal', 'Schema', 'Concept'];
const digest = (value: unknown) => createHash('sha256').update(JSON.stringify(value)).digest('hex');

export function reciprocalRankFusion(rankings: string[][], k = 60): Map<string, number> {
  const scores = new Map<string, number>();
  for (const ranking of rankings) for (const [rank, id] of [...new Set(ranking)].entries()) scores.set(id, (scores.get(id) ?? 0) + 1 / (k + rank + 1));
  return scores;
}

export function residualVector(query: number[], selected: number[][]): number[] | undefined {
  if (!selected.length) return undefined;
  const decomposition = new SingularValueDecomposition(new Matrix(selected).transpose(), { autoTranspose: true });
  const basis = decomposition.leftSingularVectors;
  const residual = [...query];
  for (let column = 0; column < decomposition.diagonal.length; column++) {
    if ((decomposition.diagonal[column] ?? 0) < 1e-8) continue;
    let projection = 0;
    for (let row = 0; row < query.length; row++) projection += (query[row] ?? 0) * basis.get(row, column);
    for (let row = 0; row < query.length; row++) residual[row] = (residual[row] ?? 0) - projection * basis.get(row, column);
  }
  const norm = Math.hypot(...residual);
  return norm < 1e-6 ? undefined : residual.map(value => value / norm);
}

function structuredMatch(view: KnowledgeView, query: Query) {
  const refNames = (refs: Ref[]) => refs.flatMap(ref => [ref.id, `${ref.kind.toLowerCase()}:${ref.id}`, sourceKey(ref)]);
  const entities = refNames(view.body.entityRefs);
  const concepts = refNames(view.body.conceptRefs);
  const match = (requested: string[], available: string[], all: boolean) => !requested.length || (all ? requested.every(item => available.includes(item)) : requested.some(item => available.includes(item)));
  if (!match(query.entities, entities, query.entityMatch === 'all')) return false;
  if (!match(query.concepts, concepts, false)) return false;
  if (!match(query.tags, view.body.tags, query.tagMatch === 'all')) return false;
  if (!match(query.refs, [view.ref.id, view.alias, sourceKey(view.ref)], false)) return false;
  if (!match(query.sources, refNames([...view.sourceRefs, ...view.sourceRoots]), false)) return false;
  if (query.time) {
    const { field, clockId, start, end } = query.time;
    if (field === 'valid') {
      if (view.valid?.start && (view.valid.start.clockId !== clockId || view.valid.start.at >= end)) return false;
      if (view.valid?.end && (view.valid.end.clockId !== clockId || view.valid.end.at <= start)) return false;
    } else {
      const point = field === 'formed' ? view.formed : field === 'recorded' ? view.recorded : undefined;
      if (point && (point.clockId !== clockId || point.at < start || point.at >= end)) return false;
    }
  }
  return true;
}

function sourceTimeMatch(time: NonNullable<Query['time']>, times: { observed: TimePoint[]; occurred: TimeRange[] }) {
  return time.field === 'observed'
    ? times.observed.some(point => point.clockId === time.clockId && point.at >= time.start && point.at < time.end)
    : times.occurred.some(range => Boolean(range.start || range.end) && (!range.start || range.start.clockId === time.clockId && range.start.at < time.end) && (!range.end || range.end.clockId === time.clockId && range.end.at > time.start));
}

export function createRetrieval(deps: RetrievalDependencies) {
  const embeddingSpace = deps.embed?.id;
  const tableName = `knowledge_${digest({ tokenizer: 'icu', embeddingSpace, dimensions: deps.embed?.dimensions }).slice(0, 16)}`;
  let connection: Promise<lance.Connection> | undefined;
  let table: Promise<lance.Table> | undefined;
  const resultSets = new Map<string, { scope: string; refs: Ref[]; at: number }>();
  const getConnection = () => connection ??= lance.connect(deps.indexPath);

  async function open(rows: Record<string, unknown>[]) {
    if (!table) table = (async () => {
      const db = await getConnection();
      const names = await db.tableNames();
      const result = names.includes(tableName) ? await db.openTable(tableName) : await db.createTable(tableName, rows, { existOk: true });
      await result.createIndex('text', { config: lance.Index.fts({ baseTokenizer: 'icu', stem: false, removeStopWords: false, asciiFolding: false, maxTokenLength: 200 }), replace: true });
      await result.updateFieldMetadata([{ path: 'text', metadata: { tokenizer: 'icu', embeddingSpace: embeddingSpace ?? 'none', dimensions: String(deps.embed?.dimensions ?? 0) } }]);
      return result;
    })().catch(error => { table = undefined; throw error; });
    return table;
  }

  async function refresh(ctx: CallContext, views?: KnowledgeView[]) {
    if(!views){let cursor:string|undefined,indexed=0;do{const page=await deps.knowledge.list(ctx,{includeHistory:true,cursor,limit:100});indexed+=(await refresh(ctx,page.items)).indexed;cursor=page.nextCursor;}while(cursor);return {textReady:true,vectorReady:Boolean(deps.embed),indexed};}
    const selected = views;
    if (!selected.length) return { textReady: true, vectorReady: Boolean(deps.embed), indexed: 0 };
    let target = table ? await table : undefined;
    if (!target && (await (await getConnection()).tableNames()).includes(tableName)) target = await open([]);
    const indexed = target ? new Set((await target.query().where(`key IN (${selected.map(view => quoted(keyOf(view))).join(',')})`).select(['key']).limit(selected.length).toArray()).map(row => String(row.key))) : new Set<string>();
    const rows: Record<string, unknown>[] = selected.filter(view => !indexed.has(keyOf(view))).map(view => ({
      key: keyOf(view), id: view.ref.id, revision: view.revision, subject_id: ctx.subjectId,
      text: `${view.body.title ?? ''}\n${view.body.kind === 'Concept' ? [view.body.name, ...view.body.aliases].join(' ') : ''}\n${view.body.content}`,
    }));
    if (!rows.length) return { textReady: true, vectorReady: Boolean(deps.embed), indexed: 0 };
    if (deps.embed) {
      for (let offset = 0; offset < rows.length; offset += 64) {
        const batch = rows.slice(offset, offset + 64);
        const vectors = await deps.embed.embed(ctx, batch.map(row => String(row.text)));
        if (vectors.length !== batch.length || vectors.some(vector => vector.length !== deps.embed?.dimensions || vector.some(n => !Number.isFinite(n)))) throw new Error('Embedding provider returned an invalid vector batch');
        for (const [index, row] of batch.entries()) row.vector = vectors[index];
      }
    }
    target ??= await open(rows);
    await target.mergeInsert('key').whenMatchedUpdateAll().whenNotMatchedInsertAll().execute(rows);
    return { textReady: true, vectorReady: Boolean(deps.embed), indexed: rows.length };
  }

  async function query(ctx: CallContext, input: QueryInput): Promise<QueryResult> {
    deps.authorize(ctx, 'knowledge.read');
    const resolved = resolveQuery(input);
    if (resolved.subjectId && resolved.subjectId !== ctx.subjectId) fail('forbidden', '查询主体与已授权主体不符');
    resolved.subjectId = ctx.subjectId;
    resolved.limit = Math.min(resolved.limit, 100);
    if (!resolved.kinds) {
      const enabled = await deps.capabilities?.(ctx);
      resolved.kinds = defaults.filter(kind => !enabled || enabled.includes(kind));
    }
    if (!resolved.kinds.length) return QueryResultSchema.parse({ items: [], resolvedQuery: resolved, resultSetId: uuid(), indexState: { textReady: true, vectorReady: Boolean(deps.embed), embeddingSpace, tokenizer: 'icu', selectedRevisions: 0, historical: Boolean(resolved.knowledgeAt || resolved.recordedAsOf) } });
    if(!table){const db=await getConnection();if((await db.tableNames()).includes(tableName))await open([]);else await refresh(ctx);}
    const target=table?await table:undefined;
    let hits:Array<Record<string,unknown>>=[];
    let queryVector:number[]|undefined;
    if(target&&resolved.intent.trim()){
      const filter=`subject_id = ${quoted(ctx.subjectId)}`;
      if(deps.embed){[queryVector]=await deps.embed.embed(ctx,[resolved.intent]);if(!queryVector||queryVector.length!==deps.embed.dimensions||queryVector.some(value=>!Number.isFinite(value)))fail('dependency_unavailable','查询嵌入维度或数值无效');const rrf=await lance.rerankers.RRFReranker.create();hits=await target.query().nearestTo(queryVector).fullTextSearch(resolved.intent).rerank(rrf).where(filter).limit(160).toArray();}
      else hits=await target.search(resolved.intent,'fts').where(filter).limit(160).toArray();
    }
    const references=[...new Set(hits.map(hit=>String(hit.id)))];
    let availableViews=references.length?await deps.knowledge.readMany(ctx,references.map(id=>({id,knowledgeAt:resolved.knowledgeAt,recordedAsOf:resolved.recordedAsOf}))):[];
    const explicit=resolved.refs.length||resolved.entities.length||resolved.concepts.length||resolved.tags.length||resolved.sources.length;
    if(explicit||!resolved.intent.trim()){
      const scope=await deps.knowledge.list(ctx,{kinds:resolved.kinds,knowledgeAt:resolved.knowledgeAt,recordedAsOf:resolved.recordedAsOf,refs:resolved.refs,entities:resolved.entities,entityMatch:resolved.entityMatch,tags:resolved.tags,tagMatch:resolved.tagMatch,sources:resolved.sources,conceptIds:resolved.concepts,limit:160});
      availableViews=[...new Map([...availableViews,...scope.items].map(view=>[keyOf(view),view])).values()];
    }
    let selected=availableViews.filter(view=>view.status==='active'&&resolved.kinds!.includes(view.kind)&&structuredMatch(view,resolved));
    const time=resolved.time;
    if(time&&(time.field==='occurred'||time.field==='observed')){
      if(!deps.sourceTimes)fail('capability_unavailable','来源时间读取尚未接入');
      const timed=await Promise.all(selected.map(async view=>sourceTimeMatch(time,await deps.sourceTimes!(ctx,view.sourceRefs))?view:undefined));selected=timed.filter((view):view is KnowledgeView=>Boolean(view));
    }
    const byKey=new Map(selected.map(view=>[keyOf(view),view]));
    const signature=digest({subject:ctx.subjectId,actor:ctx.actor,purpose:ctx.purpose,audience:ctx.audience,query:{...resolved,cursor:undefined},revisions:availableViews.map(keyOf).sort()});
    let offset=0;
    if(resolved.cursor){let cursor:{offset:number;signature:string};try{cursor=JSON.parse(Buffer.from(resolved.cursor,'base64url').toString('utf8'));}catch{fail('invalid_input','查询游标格式无效');}if(cursor.signature!==signature||!Number.isSafeInteger(cursor.offset)||cursor.offset<0)fail('conflict','查询或对象修订已变化，请重新查询');offset=cursor.offset;}
    let ranking: string[] = [];
    const reasons = new Map<string, string[]>();
    if(selected.length){
      ranking=resolved.intent.trim()?hits.map(hit=>String(hit.key)).filter(key=>byKey.has(key)):[...byKey.keys()];
      for(const id of ranking)reasons.set(id,[deps.embed?'词项与语义匹配':'中文词项匹配']);
      if(explicit){ranking=[...reciprocalRankFusion([ranking,[...byKey.keys()].slice(0,80)])].sort((a,b)=>b[1]-a[1]).map(([id])=>id);for(const id of byKey.keys())reasons.set(id,[...reasons.get(id)??[],'明确对象与范围']);}
      if(resolved.residual&&queryVector&&target){const vectors=await target.query().where(`key IN (${ranking.slice(0,8).map(quoted).join(',')||"''"})`).select(['vector']).limit(8).toArray();const residual=residualVector(queryVector,vectors.map(hit=>Array.from(hit.vector as Iterable<number>)));if(residual){const weak=await target.search(residual).where(`subject_id = ${quoted(ctx.subjectId)}`).limit(40).toArray();const views=await deps.knowledge.readMany(ctx,weak.map(hit=>({id:String(hit.id),knowledgeAt:resolved.knowledgeAt,recordedAsOf:resolved.recordedAsOf})));const valid=views.filter(view=>view.status==='active'&&resolved.kinds!.includes(view.kind)&&structuredMatch(view,resolved));for(const view of valid)byKey.set(keyOf(view),view);const ids=weak.map(hit=>String(hit.key)).filter(key=>byKey.has(key));ranking=[...reciprocalRankFusion([ranking,ids])].sort((a,b)=>b[1]-a[1]).map(([id])=>id);for(const id of ids)reasons.set(id,[...reasons.get(id)??[],'弱线索残差发现']);}}
      if (resolved.exploration && deps.relations) {
        const graph = new MultiDirectedGraph();
        let frontier = ranking.slice(0, 12).map(id => byKey.get(id)!.ref);
        for (let hop = 0; hop < resolved.exploration && graph.order < 60 && frontier.length; hop++) {
          const edges = await deps.relations(ctx, frontier, 60 - graph.order,{knowledgeAt:resolved.knowledgeAt,recordedAsOf:resolved.recordedAsOf});
          const next: Ref[] = [];
          const endpoints=await deps.knowledge.readMany(ctx,[...new Set(edges.flatMap(edge=>[edge.from.id,edge.to.id]))].map(id=>({id,knowledgeAt:resolved.knowledgeAt,recordedAsOf:resolved.recordedAsOf})));
          for (const edge of edges) {
            try {
              const from=endpoints.find(view=>view.ref.id===edge.from.id),to=endpoints.find(view=>view.ref.id===edge.to.id);if(!from||!to)continue;
              if (from.status !== 'active' || to.status !== 'active') continue;
            } catch (error) { if (error instanceof DomainError && ['forbidden', 'not_found'].includes(error.code)) continue; throw error; }
            for (const ref of [edge.from, edge.to]) if (!graph.hasNode(ref.id)) { graph.addNode(ref.id, { ref }); next.push(ref); }
            graph.addDirectedEdge(edge.from.id, edge.to.id, { weight: edge.weight * 0.6 ** (hop + 1), relation: edge.relation });
            for (const ref of [edge.from, edge.to]) {
              const related = endpoints.find(view => view.ref.id === ref.id && view.status === 'active' && structuredMatch(view, { ...resolved, refs: [] }));
              if (related && time && (time.field === 'occurred' || time.field === 'observed') && !sourceTimeMatch(time, await deps.sourceTimes!(ctx, related.sourceRefs))) continue;
              if (related && !ranking.includes(keyOf(related))) { byKey.set(keyOf(related), related); ranking.push(keyOf(related)); reasons.set(keyOf(related), [`关联 ${edge.relation}，第 ${hop + 1} 跳`]); }
            }
          }
          frontier = next;
        }
      }
    }
    if (resolved.preferences.includes('recent')) ranking.sort((a, b) => byKey.get(b)!.formed.at.localeCompare(byKey.get(a)!.formed.at));
    if (resolved.preferences.some(p => p === 'independent_sources' || p === 'diverse')) {
      const seen = new Set<string>();
      const repeats: string[] = [];
      ranking = ranking.filter(id => {
        const roots = byKey.get(id)!.sourceRoots.map(sourceKey);
        const repeated = roots.length > 0 && roots.every(root => seen.has(root));
        roots.forEach(root => seen.add(root));
        if (repeated) repeats.push(id);
        return !repeated;
      }).concat(repeats);
    }
    const chosen = ranking.slice(offset, offset + resolved.limit).map(id => byKey.get(id)).filter((view): view is KnowledgeView => Boolean(view));
    const resultSetId = uuid();
    resultSets.set(resultSetId, { scope: digest([ctx.subjectId, ctx.actor.id, ctx.purpose, ctx.audience]), refs: chosen.map(view => view.ref), at: Date.now() });
    for (const [id, value] of resultSets) if (Date.now() - value.at > 3_600_000 || resultSets.size > 1000) resultSets.delete(id);
    return QueryResultSchema.parse({
      items: chosen.map(view => ({ ref: view.ref, title: view.body.title ?? (view.body.kind === 'Concept' ? view.body.name : view.kind), excerpt: view.body.content, formation: view.formation, stance: view.stance, confidence: view.confidence, time: { formed: view.formed, valid: view.valid, recorded: view.recorded }, sourceRefs: view.sourceRefs, matchReasons: reasons.get(keyOf(view)) ?? ['范围内认识'] })),
      resolvedQuery: resolved, resultSetId,
      nextCursor: offset + chosen.length < ranking.length ? Buffer.from(JSON.stringify({ offset: offset + chosen.length, signature })).toString('base64url') : undefined,
      indexState: { textReady: true, vectorReady: Boolean(deps.embed), embeddingSpace, tokenizer: 'icu', selectedRevisions: selected.length, historical: Boolean(resolved.knowledgeAt || resolved.recordedAsOf) },
    });
  }

  async function readResult(ctx: CallContext, resultSetId: string, number: number) {
    const set = resultSets.get(resultSetId);
    if (!set || set.scope !== digest([ctx.subjectId, ctx.actor.id, ctx.purpose, ctx.audience])) fail('not_found', '查询结果集不存在');
    const ref = set.refs[number - 1];
    if (!ref) fail('invalid_input', '查询结果编号不存在');
    return deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision });
  }

  async function expand(ctx: CallContext, input: z.input<typeof ExpandSchema>) {
    deps.authorize(ctx, 'knowledge.read');
    const request = ExpandSchema.parse(input);
    const items = new Map<string, KnowledgeView>();
    const sources = new Map<string, MaterialView>();
    const relations: Array<{ from: Ref; to: Ref; relation: string; weight: number }> = [];
    let frontier = request.refs;
    const seen = new Set<string>();
    for (let hop = 0; hop < request.depth && frontier.length && items.size + sources.size < request.limit; hop++) {
      const next: Ref[] = [];
      for (const ref of frontier) {
        const key = canonicalRefKey(ref);
        if (seen.has(key) || items.size + sources.size >= request.limit) continue;
        seen.add(key);
        if (ref.kind === 'Material') {
          if (!deps.materials) fail('capability_unavailable', '来源材料读取尚未配置');
          const material = await deps.materials.read(ctx, { ref });
          sources.set(key, material);
        } else if (ref.kind === 'Encounter' || ref.kind === 'ExperienceItem') {
          if (!deps.encounterInputs) fail('capability_unavailable', '实际接触来源读取尚未配置');
          next.push(...await deps.encounterInputs(ctx, ref));
        } else {
          const item = await deps.knowledge.read(ctx, { id: ref.id, revision: ref.revision });
          items.set(key, item);
          if (request.include.includes('sources')) next.push(...item.sourceRefs);
        }
      }
      if (request.include.includes('relations') && deps.relations) {
        for (const edge of await deps.relations(ctx, frontier, request.limit - items.size - sources.size)) {
          // Both endpoints pass the authority reader before their relation is disclosed.
          await deps.knowledge.read(ctx, { id: edge.from.id, revision: edge.from.revision });
          await deps.knowledge.read(ctx, { id: edge.to.id, revision: edge.to.revision });
          relations.push(edge);
          next.push(edge.from, edge.to);
        }
      }
      // Sources requested at depth 1 include the direct sources of the selected object.
      if (hop === request.depth - 1 && request.include.includes('sources') && deps.materials) for (const ref of next) {
        if ((ref.kind === 'Encounter' || ref.kind === 'ExperienceItem') && deps.encounterInputs) {
          for (const portion of await deps.encounterInputs(ctx, ref)) if (portion.kind === 'Material' && items.size + sources.size < request.limit) {
            const key = canonicalRefKey(portion);
            if (!sources.has(key)) sources.set(key, await deps.materials.read(ctx, { ref: portion }));
          }
          continue;
        }
        if (ref.kind !== 'Material' || items.size + sources.size >= request.limit) continue;
        const key = canonicalRefKey(ref);
        if (!sources.has(key)) sources.set(key, await deps.materials.read(ctx, { ref }));
      }
      frontier = next;
    }
    return ExpansionResultSchema.parse({
      items: [...items.values()].map(view => ({ ref: view.ref, title: view.body.title ?? view.kind, excerpt: view.body.content, formation: view.formation, stance: view.stance, confidence: view.confidence, time: { formed: view.formed, valid: view.valid, recorded: view.recorded }, sourceRefs: view.sourceRefs, matchReasons: ['明确来源展开'] })),
      sources: [...sources.values()], relations,
    });
  }

  return {
    query, refresh, readResult, expand,
    rebuild: async (ctx: CallContext) => { deps.authorize(ctx, 'maintenance.run'); const db = await getConnection(); if ((await db.tableNames()).includes(tableName)) await (await db.openTable(tableName)).delete(`subject_id = ${quoted(ctx.subjectId)}`); return refresh(ctx); },
    explain: async (ctx: CallContext, input: QueryInput) => { const result = await query(ctx, input); return { query: result.resolvedQuery, indexState: result.indexState, matches: result.items.map(item => ({ ref: item.ref, reasons: item.matchReasons })) }; },
    removeSubject: async (ctx: CallContext) => { deps.authorize(ctx, 'data.purge'); const db = await getConnection(); for (const name of await db.tableNames()) if (name.startsWith('knowledge_')) await (await db.openTable(name)).delete(`subject_id = ${quoted(ctx.subjectId)}`); },
    close: async () => { if (table) (await table).close(); if (connection) (await connection).close(); resultSets.clear(); },
  };
}
