import { createHash } from 'node:crypto';
import { readFile, readdir, realpath, stat } from 'node:fs/promises';
import { extname, isAbsolute, relative, resolve, sep } from 'node:path';
import { v7 as uuid } from 'uuid';
import type { CallContext, Ref, Selector } from '@nous-wave/contracts/common';
import { ResourceBodySchema, type ResourceBody } from '@nous-wave/contracts/cognition';
import type { MaterialView } from '@nous-wave/contracts/materials';
import { authorize, mayRead } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { json, now, reference, type Storage } from '../platform/context.js';

export interface ResourceDependencies extends Storage {
  materials: {
    capture(ctx: CallContext, input: unknown): Promise<MaterialView>;
    read(ctx: CallContext, input: { ref: Ref }): Promise<MaterialView>;
    putBlob(ctx: CallContext, bytes: Buffer, mediaType: string): Promise<{ ref: Ref }>;
  };
  mcp?: Record<string, {
    list?(ctx: CallContext): Promise<Array<{ uri: string; title?: string }>>;
    search?(ctx: CallContext, query: string): Promise<Array<{ uri: string; title?: string }>>;
    read(ctx: CallContext, uri: string): Promise<{ text: string; mediaType: string; version?: string; title?: string }>;
  }>;
}

const hash = (text: string | Buffer) => createHash('sha256').update(text).digest('hex');
const mediaType = (name: string) => ({ '.md': 'text/markdown', '.txt': 'text/plain', '.json': 'application/json', '.html': 'text/html', '.csv': 'text/csv', '.toml': 'application/toml', '.yaml': 'application/yaml', '.yml': 'application/yaml', '.xml': 'application/xml' })[extname(name).toLowerCase()] ?? 'application/octet-stream';

export function createResources(deps: ResourceDependencies) {
  async function inspect(ctx: CallContext, input: { id: string }) {
    authorize(ctx, 'resources.read');
    const row = await deps.db.selectFrom('resources').selectAll().where('id', '=', input.id).where('subjectId', '=', ctx.subjectId).executeTakeFirst();
    if (!row) fail('not_found', '外部资料源不存在');
    const body = ResourceBodySchema.parse(row.body);
    if (!mayRead(ctx, body.policy)) fail('forbidden', '资料源不允许本次用途或听众');
    return { ref: reference(deps.installationId, ctx.subjectId, 'Resource', row.id, row.revision), ...body, recordedAt: new Date(row.recordedAt).toISOString() };
  }

  async function pathWithin(root: string, target: string) {
    const canonicalRoot = await realpath(root);
    const path = await realpath(resolve(canonicalRoot, target));
    const suffix = relative(canonicalRoot, path);
    if (suffix === '..' || suffix.startsWith(`..${sep}`) || isAbsolute(suffix)) fail('forbidden', '读取目标超出资料源目录');
    return { path, relative: suffix };
  }

  function checkUrl(resource: Extract<ResourceBody, { kind: 'http' }>, input: string): URL {
    const url = new URL(input, resource.baseUrl);
    const origins = [new URL(resource.baseUrl).origin, ...resource.allowedOrigins.map(origin => new URL(origin).origin)];
    if (!['http:', 'https:'].includes(url.protocol) || !origins.includes(url.origin) || url.username || url.password) fail('forbidden', '读取目标超出资料源授权 origin');
    return url;
  }

  async function httpRead(resource: Extract<ResourceBody, { kind: 'http' }>, target: string) {
    let url = checkUrl(resource, target);
    for (let redirects = 0; redirects <= 5; redirects++) {
      const response = await fetch(url, { redirect: 'manual', signal: AbortSignal.timeout(30_000) });
      if ([301, 302, 303, 307, 308].includes(response.status)) {
        const location = response.headers.get('location');
        await response.body?.cancel();
        if (!location) fail('dependency_unavailable', '资料响应缺少重定向位置');
        url = checkUrl(resource, new URL(location, url).toString());
        continue;
      }
      if (!response.ok || !response.body) fail('dependency_unavailable', `资料源 HTTP ${response.status}`);
      const declared = Number(response.headers.get('content-length') ?? 0);
      if (declared > resource.maxBytes) { await response.body.cancel(); fail('budget_exhausted', '资料超过单件读取大小'); }
      const chunks: Uint8Array[] = [];
      let size = 0;
      for await (const chunk of response.body) {
        size += chunk.length;
        if (size > resource.maxBytes) { await response.body.cancel().catch(() => {}); fail('budget_exhausted', '资料超过单件读取大小'); }
        chunks.push(chunk);
      }
      return { bytes: Buffer.concat(chunks), mediaType: response.headers.get('content-type')?.split(';')[0] ?? mediaType(url.pathname), title: url.pathname, target: url.toString(), originVersion: response.headers.get('etag') ?? response.headers.get('last-modified') ?? undefined };
    }
    fail('dependency_unavailable', '资料源重定向超过上限');
  }

  function connector(resource: Extract<ResourceBody, { kind: 'mcp' }>) {
    const selected = deps.mcp?.[resource.connectorId];
    if (!selected) fail('capability_unavailable', '资料源 MCP 连接尚未配置');
    return selected;
  }

  return {
    async register(ctx: CallContext, input: unknown) {
      authorize(ctx, 'resources.register');
      const body = ResourceBodySchema.parse(input);
      if (body.kind === 'directory') body.root = await realpath(body.root);
      if (body.kind === 'http') checkUrl(body, body.baseUrl);
      if (body.kind === 'mcp') connector(body);
      return deps.transaction(async () => {
        const id = uuid();
        await deps.db.insertInto('resources').values({ id, subjectId: ctx.subjectId, revision: 1, body: json(body), recordedAt: now().at }).execute();
        return inspect(ctx, { id });
      });
    },
    inspect,
    async list(ctx: CallContext) {
      authorize(ctx, 'resources.read');
      const rows = await deps.db.selectFrom('resources').select(['id', 'body']).where('subjectId', '=', ctx.subjectId).execute();
      return Promise.all(rows.filter(row => mayRead(ctx, ResourceBodySchema.parse(row.body).policy)).map(row => inspect(ctx, row)));
    },
    async directory(ctx: CallContext, input: { id: string; target?: string; query?: string }) {
      const resource = await inspect(ctx, input);
      if (resource.kind === 'directory') {
        const { path } = await pathWithin(resource.root, input.target ?? '.');
        const entries = await readdir(path, { withFileTypes: true });
        return entries.filter(entry => (entry.isDirectory() || !resource.extensions.length || resource.extensions.includes(extname(entry.name))) && (!input.query || entry.name.includes(input.query))).map(entry => ({ target: relative(resource.root, resolve(path, entry.name)), title: entry.name, kind: entry.isDirectory() ? 'directory' : 'file' }));
      }
      if (resource.kind === 'mcp') {
        const client = connector(resource);
        const entries = input.query && client.search ? await client.search(ctx, input.query) : await client.list?.(ctx);
        if (!entries) fail('capability_unavailable', '此资料源没有目录或搜索能力');
        return entries.filter(entry => !resource.allowedUris.length || resource.allowedUris.some(prefix => entry.uri.startsWith(prefix))).map(entry => ({ ...entry, target: entry.uri, kind: 'file' }));
      }
      return [{ target: resource.baseUrl, title: resource.title, kind: 'document' }];
    },
    async read(ctx: CallContext, input: { id: string; target: string; version?: string; selector?: Selector }) {
      const resource = await inspect(ctx, input);
      if (input.version) {
        const saved = await deps.db.selectFrom('resourceVersions').select('materialRef').where('resourceId', '=', resource.ref.id).where('version', '=', input.version).executeTakeFirst();
        if (!saved) fail('not_found', '资料源的该版本尚未取得');
        return deps.materials.read(ctx, { ref: saved.materialRef });
      }
      let content: { bytes: Buffer; mediaType: string; title: string; target: string; originVersion?: string };
      if (resource.kind === 'directory') {
        const location = await pathWithin(resource.root, input.target);
        if (resource.extensions.length && !resource.extensions.includes(extname(location.path))) fail('forbidden', '文件类型超出资料源授权');
        const info = await stat(location.path);
        if (!info.isFile()) fail('invalid_input', '目标不是普通文件');
        if (info.size > resource.maxBytes) fail('budget_exhausted', '资料超过单件读取大小');
        const bytes = await readFile(location.path);
        if (bytes.length > resource.maxBytes) fail('budget_exhausted', '资料超过单件读取大小');
        content = { bytes, mediaType: mediaType(location.path), title: location.relative, target: location.relative, originVersion: `${info.mtimeMs}:${info.size}` };
      } else if (resource.kind === 'http') content = await httpRead(resource, input.target);
      else {
        if (resource.allowedUris.length && !resource.allowedUris.some(prefix => input.target.startsWith(prefix))) fail('forbidden', 'MCP URI 超出授权范围');
        const result = await connector(resource).read(ctx, input.target);
        const bytes = Buffer.from(result.text);
        if (bytes.length > resource.maxBytes) fail('budget_exhausted', '资料超过单件读取大小');
        content = { bytes, mediaType: result.mediaType, title: result.title ?? input.target, target: input.target, originVersion: result.version };
      }
      const version = hash(Buffer.concat([Buffer.from(content.target), Buffer.from('\0'), content.bytes]));
      const existing = await deps.db.selectFrom('resourceVersions').select('materialRef').where('resourceId', '=', resource.ref.id).where('version', '=', version).executeTakeFirst();
      if (existing) return deps.materials.read(ctx, { ref: existing.materialRef });
      const isText = /^(text\/|application\/(json|toml|yaml|xml))/.test(content.mediaType);
      const blob = isText ? undefined : await deps.materials.putBlob(ctx, content.bytes, content.mediaType);
      return deps.transaction(async () => {
        await deps.db.selectFrom('resources').select('id').where('id', '=', resource.ref.id).forUpdate().executeTakeFirstOrThrow();
        const prior = await deps.db.selectFrom('resourceVersions').select('materialRef').where('resourceId', '=', resource.ref.id).where('version', '=', version).executeTakeFirst();
        if (prior) return deps.materials.read(ctx, { ref: { ...prior.materialRef, selector: input.selector } });
        const id = uuid();
        const sourceRef: Ref = reference(deps.installationId, ctx.subjectId, 'ResourceVersion', id);
        const material = await deps.materials.capture(ctx, { kind: 'resource', mediaType: content.mediaType, title: content.title, text: isText ? content.bytes.toString('utf8') : undefined, blobRef: blob?.ref, source: { ref: sourceRef, roots: [sourceRef], originVersion: content.originVersion ?? version }, policy: resource.policy });
        await deps.db.insertInto('resourceVersions').values({ id, resourceId: resource.ref.id, version, materialRef: json(material.ref), recordedAt: now().at }).execute();
        return deps.materials.read(ctx, { ref: { ...material.ref, selector: input.selector } });
      });
    },
  };
}
