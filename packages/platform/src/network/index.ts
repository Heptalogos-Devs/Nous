import {
  EnvironmentProxyRoute,
  environmentProxySnapshot,
  type EnvironmentProxySnapshot,
} from './environment.js';
import { createHash } from 'node:crypto';
import { isIP } from 'node:net';
import { lookup } from 'node:dns/promises';
import { LRUCache } from 'lru-cache';
import { Dispatcher1Wrapper, WebSocket, type Dispatcher, util } from 'undici';
import {
  canonicalRefKey,
  RefSchema,
  TimePointInputSchema,
  type CallContext,
  type Ref,
} from '@nous-wave/contracts/common';
import { problem, ProblemError } from '../errors.js';
import { canonicalize } from 'json-canonicalize';
import type { ConfigScope, ConfigSnapshot } from '../settings/index.js';
import { networkSettingsModule } from './config.js';
import {
  NetworkSettingsSchema,
  ResourceNetworkScopeSchema,
  type NetworkOptions,
  type NetworkSelection,
  type NetworkMetadata,
  type NetworkProfile,
  type ResourceNetworkScope,
} from './contracts.js';
import { requestUrl, authorizedUrl, contentLength } from './url.js';
import { allowedAddress, assertLiteralAddress } from './address.js';
import {
  createTransport,
  materialRefs,
  withMaterials,
  type Materials,
} from './transport.js';
import { networkError } from './errors.js';
import type { SecretUse } from '@nous-wave/contracts/credentials';
export { networkSettingsModule };
export * from './contracts.js';
interface PoolEntry {
  environment?: EnvironmentProxySnapshot;
  scopeKey: string;
  agent: Dispatcher;
  key: string;
  resourceRef: Ref;
  users: number;
  retired: boolean;
  done: PromiseWithResolvers<void>;
  closed: Promise<void>;
  versions: string;
}
const digest = (value: unknown) =>
  createHash('sha256').update(canonicalize(value)).digest('hex');
function versions(materials: Materials) {
  return digest(
    [...materials].map(([ref, value]) => [
      ref,
      value.metadata.revision,
      value.metadata.generation,
    ]),
  );
}
function objectKey(ref: Ref) {
  return canonicalize([
    ref.product,
    ref.installationId,
    ref.subjectId ?? null,
    ref.kind,
    ref.id,
  ]);
}
function includesScope(parent: ConfigScope, child: ConfigScope) {
  return (
    parent.kind === 'installation' ||
    (parent.kind === 'subject' &&
      child.kind !== 'installation' &&
      parent.subjectId === child.subjectId) ||
    canonicalize(parent) === canonicalize(child)
  );
}
function headers(input: Dispatcher.DispatchOptions['headers']): Headers {
  return new Headers(
    (Array.isArray(input) && typeof input[0] === 'string'
      ? util.parseHeaders(input as string[])
      : (input ?? {})) as HeadersInit,
  );
}
export function createNetwork(options: NetworkOptions) {
  let closed = false;
  const leases = new Set<NetworkClient>();
  const consumers = new Map<
    string,
    ReturnType<NetworkOptions['settings']['registerConsumer']>
  >();
  const pending = new Set<Promise<void>>();
  function retire(entry: PoolEntry) {
    entry.retired = true;
    if (entry.users === 0) entry.done.resolve();
  }
  const pools = new LRUCache<string, PoolEntry>({
    max: options.maxPools ?? 32,
    disposeAfter: retire,
  });
  const unsubscribe = options.settings.subscribe((event) => {
    if (
      event.kind === 'resync' ||
      event.change.namespace === networkSettingsModule.id
    )
      pools.clear();
  });
  function consumer(scope: ConfigScope) {
    const key = canonicalize(scope);
    let value = consumers.get(key);
    if (!value) {
      value = options.settings.registerConsumer({
        namespace: networkSettingsModule.id,
        consumerId: 'network.factory',
        processBootId: options.processBootId,
        scope,
      });
      consumers.set(key, value);
    }
    return value;
  }
  function profileReference(profileId: string): Ref {
    return {
      product: options.product,
      installationId: options.installationId,
      kind: 'NetworkProfile',
      id: profileId,
    };
  }
  function selectionScope(
    ctx: CallContext,
    selection: NetworkSelection,
  ): ConfigScope {
    const scope =
      selection.scope ??
      (ctx.subjectId
        ? { kind: 'subject' as const, subjectId: ctx.subjectId }
        : {
            kind: 'installation' as const,
            installationId: options.installationId,
          });
    if (
      (scope.kind === 'installation' &&
        scope.installationId !== options.installationId) ||
      (scope.kind !== 'installation' && ctx.subjectId !== scope.subjectId)
    )
      throw problem(
        'network.scope_mismatch',
        'forbidden',
        '网络配置范围与当前主体或安装不符',
      );
    return scope;
  }
  async function selectedProfile(
    ctx: CallContext,
    selection: NetworkSelection,
  ) {
    const scope = selectionScope(ctx, selection),
      registered = consumer(scope),
      snapshot = await registered.snapshot(),
      settings = NetworkSettingsSchema.parse(snapshot.values),
      profileId = selection.resource.profileId ?? settings.defaultProfileId,
      profile = settings.profiles[profileId];
    if (!profile)
      throw problem(
        'network.profile_not_found',
        'not_found',
        '网络profile不存在',
      );
    return {
      scope,
      registered,
      snapshot,
      profileId,
      profile,
      profileRef: profileReference(profileId),
    };
  }
  async function resolveResource(ctx: CallContext, useRef: Ref) {
    if (!options.resolveResourceNetwork)
      throw problem(
        'network.resource_owner_unavailable',
        'unavailable',
        '网络材料需要实际资源owner重新核验',
      );
    const selection = await options.resolveResourceNetwork(ctx, useRef);
    const resource = ResourceNetworkScopeSchema.parse(selection.resource);
    if (
      canonicalRefKey(resource.resourceRef) !== canonicalRefKey(useRef) ||
      resource.resourceRef.product !== options.product ||
      resource.resourceRef.installationId !== options.installationId
    )
      throw problem(
        'network.unauthorized_target',
        'forbidden',
        '网络资源owner返回了不同对象',
      );
    return { ...selection, resource };
  }
  async function credentialTarget(
    ctx: CallContext,
    input: SecretUse & { useRef: Ref },
  ) {
    const selection = await resolveResource(ctx, input.useRef),
      selected = await selectedProfile(ctx, selection);
    if (
      canonicalRefKey(input.resourceRef) !==
        canonicalRefKey(selected.profileRef) ||
      !materialRefs(selected.profile).some(
        (item) => item.ref === input.ref && item.purpose === input.purpose,
      )
    )
      throw problem(
        'network.credential_mismatch',
        'forbidden',
        '网络材料不属于实际资源当前选择的profile',
      );
    return {
      operation: selection.resource.operation,
      target: selection.resource.resourceRef,
    };
  }
  async function acquire(
    ctx: CallContext,
    resource: ResourceNetworkScope,
    profileRef: Ref,
    profile: NetworkProfile,
    snapshot: ConfigSnapshot,
    scope: ConfigScope,
  ): Promise<PoolEntry> {
    const ready = Promise.withResolvers<PoolEntry>();
    const lifetime = withMaterials(
      options,
      ctx,
      resource,
      profileRef,
      profile,
      async (materials) => {
        const materialVersions = versions(materials);
        const environment =
          profile.proxy.mode === 'environment'
            ? environmentProxySnapshot(profile.proxy.noProxy)
            : undefined;
        const key = digest([
          environment,
          canonicalRefKey(resource.resourceRef),
          resource,
          ctx.actor.authorityRef,
          ctx.consumer,
          profile,
          snapshot.definitionRevision,
          snapshot.revisions,
          materialVersions,
        ]);
        const prior = pools.get(key);
        if (prior && !prior.retired) {
          ready.resolve(prior);
          return;
        }
        const entry: PoolEntry = {
          environment,
          scopeKey: canonicalize(scope),
          agent: createTransport(profile, resource, materials, environment),
          key,
          resourceRef: resource.resourceRef,
          users: 0,
          retired: false,
          done: Promise.withResolvers<void>(),
          closed: Promise.resolve(),
          versions: materialVersions,
        };
        entry.closed = entry.done.promise.then(() => entry.agent.close());
        pools.set(key, entry);
        ready.resolve(entry);
        await entry.closed;
      },
    );
    pending.add(lifetime);
    void lifetime.catch(ready.reject).finally(() => pending.delete(lifetime));
    return ready.promise;
  }
  async function client(
    ctx: CallContext,
    selection: NetworkSelection,
  ): Promise<NetworkClient> {
    if (closed)
      throw problem('network.closed', 'unavailable', '网络服务已关闭');
    const resource = ResourceNetworkScopeSchema.parse({
      ...selection.resource,
      resourceRef: RefSchema.parse(selection.resource.resourceRef),
    });
    if (
      resource.resourceRef.product !== options.product ||
      resource.resourceRef.installationId !== options.installationId
    )
      throw problem(
        'network.unauthorized_target',
        'forbidden',
        '网络资源不属于当前安装',
      );
    await options.authorize(ctx, resource.operation, resource.resourceRef);
    const { scope, registered, snapshot, profileId, profile, profileRef } =
      await selectedProfile(ctx, { ...selection, resource });
    if (selection.deadline)
      selection = {
        ...selection,
        deadline: TimePointInputSchema.parse(selection.deadline),
      };
    if (selection.deadline && selection.deadline.domainId !== 'real')
      throw problem(
        'network.invalid_deadline',
        'validation',
        '网络截止时间必须使用real时域',
      );
    const pool = await acquire(
      ctx,
      resource,
      profileRef,
      profile,
      snapshot,
      scope,
    );
    pool.users++;
    const ownerAbort = new AbortController(),
      sockets = new Set<WebSocket>();
    let clientClosed = false;
    const metadata: NetworkMetadata = {
      scope: structuredClone(scope),
      resourceRef: resource.resourceRef,
      profileId,
      revisionRefs: structuredClone(snapshot.revisions),
      settingsDefinitionRevision: snapshot.definitionRevision,
    };
    const environmentRoute = pool.environment
      ? new EnvironmentProxyRoute(pool.environment)
      : undefined;
    async function current(url: URL, method: string) {
      if (clientClosed || closed)
        throw problem('network.closed', 'unavailable', '网络客户端已关闭');
      if (options.resolveResourceNetwork) {
        const actual = await resolveResource(ctx, resource.resourceRef);
        const selected = await selectedProfile(ctx, actual);
        if (
          digest(actual.resource) !== digest(resource) ||
          canonicalize(selected.scope) !== canonicalize(scope) ||
          digest(selected.profile) !== digest(profile)
        )
          throw problem(
            'network.resource_changed',
            'conflict',
            '网络资源或profile已变化，请使用新客户端',
          );
      }
      authorizedUrl(resource, url, method);
      assertLiteralAddress(resource, url.hostname);
      await options.authorize(ctx, resource.operation, resource.resourceRef);
      const transportURL = new URL(url.href);
      if (transportURL.protocol === 'ws:') transportURL.protocol = 'http:';
      if (transportURL.protocol === 'wss:') transportURL.protocol = 'https:';
      const proxy =
        profile.proxy.mode === 'environment'
          ? environmentRoute?.select(transportURL)
          : profile.proxy.mode === 'direct'
            ? undefined
            : profile.proxy.url;
      if (proxy && !isIP(url.hostname.replace(/^\[|\]$/g, ''))) {
        if (!options.authorizeProxyRoute)
          throw problem(
            'network.scope_unavailable',
            'unavailable',
            '所选代理无法确认远端DNS地址范围',
          );
        await options.authorizeProxyRoute(ctx, { profile, resource, url });
      }
      await withMaterials(
        options,
        ctx,
        resource,
        profileRef,
        profile,
        async (materials) => {
          if (versions(materials) !== pool.versions)
            throw problem(
              'network.credentials_changed',
              'conflict',
              '网络凭据代际已变化，请使用新客户端',
            );
        },
      );
    }
    function dispatchFor(initial: URL, signal: AbortSignal) {
      let calls = 0;
      const gate: Dispatcher.DispatcherComposeInterceptor =
        (dispatch) => (request, handler) => {
          void Promise.resolve()
            .then(async () => {
              signal.throwIfAborted();
              const url = new URL(request.path ?? '/', String(request.origin));
              if (++calls > profile.redirects.maxHops + 1)
                throw problem(
                  'network.redirect_limit',
                  'forbidden',
                  '网络重定向超过资源限制',
                );
              await current(url, String(request.method ?? 'GET'));
              signal.throwIfAborted();
              const h = headers(request.headers);
              if (h.has('host') && h.get('host') !== url.host)
                throw problem(
                  'network.unauthorized_target',
                  'forbidden',
                  'Host头与授权目标不符',
                );
              h.delete('proxy-authorization');
              if (url.origin !== initial.origin) {
                h.delete('authorization');
                h.delete('cookie');
              }
              if (
                (h.has('authorization') || h.has('cookie')) &&
                !resource.credentialOrigins.includes(url.origin)
              )
                throw problem(
                  'network.unauthorized_target',
                  'forbidden',
                  '该目标未获准携带凭据',
                );
              registered.reportApplied(snapshot);
              dispatch(
                {
                  ...request,
                  headers: Object.fromEntries(h),
                  headersTimeout: profile.timeout.headersMs,
                  bodyTimeout: profile.timeout.bodyIdleMs,
                },
                handler,
              );
            })
            .catch((error) => {
              // Undici's own asynchronous DNS interceptor uses null before a connector exists.
              handler.onResponseError?.(
                null as unknown as Dispatcher.DispatchController,
                networkError(error, 'dispatch', signal),
              );
            });
          return true;
        };
      return pool.agent.compose(gate);
    }
    const resourceFetch: typeof fetch = async (input, init) => {
      const request = new Request(input, init),
        url = requestUrl(request);
      authorizedUrl(resource, url, request.method);
      const controller = new AbortController();
      const total = Math.min(
        profile.timeout.totalMs,
        selection.deadline
          ? Math.max(0, Date.parse(selection.deadline.at) - Date.now())
          : Infinity,
      );
      if (total <= 0)
        throw problem('network.timeout', 'unavailable', '网络请求截止时间已到');
      const timeout = AbortSignal.timeout(total),
        signal = AbortSignal.any([
          request.signal,
          ownerAbort.signal,
          controller.signal,
          timeout,
        ]);
      const diagnostic = {
        resourceRef: resource.resourceRef,
        profileId,
        phase: 'prepare' as const,
        bytesRead: 0,
      };
      let bytesRead = 0,
        status: number | undefined,
        finished = false;
      const report = (
        phase: Parameters<
          NonNullable<NetworkOptions['onDiagnostic']>
        >[0]['phase'],
        error?: unknown,
      ) =>
        options.onDiagnostic?.({
          ...diagnostic,
          phase,
          bytesRead,
          ...(status ? { status } : {}),
          ...(error
            ? { problem: networkError(error, phase, signal).problem }
            : {}),
        });
      let cancel: (() => void) | undefined;
      const complete = () => {
        if (finished) return;
        finished = true;
        signal.removeEventListener('abort', aborted);
      };
      const aborted = () => {
        cancel?.();
        complete();
      };
      signal.addEventListener('abort', aborted, { once: true });
      try {
        signal.throwIfAborted();
        report('dispatch');
        const dispatcher = new Dispatcher1Wrapper(dispatchFor(url, signal));
        const response = await globalThis.fetch(
          new Request(request, {
            signal,
            redirect:
              profile.redirects.mode === 'deny' ? 'manual' : request.redirect,
          }),
          { dispatcher } as RequestInit,
        );
        status = response.status;
        report('headers');
        if (
          profile.redirects.mode === 'deny' &&
          response.status >= 300 &&
          response.status < 400
        ) {
          await response.body?.cancel();
          throw problem(
            'network.redirect_denied',
            'forbidden',
            '网络资源不允许重定向',
          );
        }
        if ((contentLength(response.headers) ?? 0) > profile.maxResponseBytes) {
          await response.body?.cancel();
          throw problem(
            'network.response_too_large',
            'resource_pressure',
            '网络响应超过字节上限',
          );
        }
        if (!response.body) {
          complete();
          report('complete');
          return response;
        }
        const reader = response.body.getReader();
        const body = new ReadableStream<Uint8Array>({
          start(destination) {
            cancel = () => {
              void reader.cancel().catch(() => {});
              destination.error(networkError(signal.reason, 'body', signal));
            };
            if (signal.aborted) aborted();
          },
          async pull(destination) {
            try {
              const chunk = await reader.read();
              if (chunk.done) {
                complete();
                report('complete');
                destination.close();
                return;
              }
              bytesRead += chunk.value.byteLength;
              if (bytesRead > profile.maxResponseBytes) {
                const exceeded = problem(
                  'network.response_too_large',
                  'resource_pressure',
                  '网络响应超过字节上限',
                );
                controller.abort(exceeded);
                await reader.cancel(exceeded);
                throw exceeded;
              }
              report('body');
              destination.enqueue(chunk.value);
            } catch (error) {
              complete();
              report('body', error);
              destination.error(networkError(error, 'body', signal));
            }
          },
          async cancel() {
            controller.abort();
            try {
              await reader.cancel();
            } finally {
              complete();
            }
          },
        });
        const output = new Response(body, {
          status: response.status,
          statusText: response.statusText,
          headers: response.headers,
        });
        function metadataOf(value: Response): Response {
          Object.defineProperties(value, {
            url: { value: response.url },
            redirected: { value: response.redirected },
            type: { value: response.type },
            clone: {
              value: () => metadataOf(Response.prototype.clone.call(value)),
            },
          });
          return value;
        }
        return metadataOf(output);
      } catch (error) {
        complete();
        report('dispatch', error);
        throw networkError(error, 'dispatch', signal);
      }
    };
    const result: NetworkClient = {
      metadata,
      fetch: resourceFetch,
      async webSocket(url, init = {}) {
        const target = requestUrl(url);
        await current(target, 'GET');
        const total = Math.min(
          profile.timeout.totalMs,
          selection.deadline
            ? Math.max(0, Date.parse(selection.deadline.at) - Date.now())
            : Infinity,
        );
        if (total <= 0)
          throw problem(
            'network.timeout',
            'unavailable',
            '网络请求截止时间已到',
          );
        const signal = AbortSignal.any([
          ownerAbort.signal,
          init.signal ?? new AbortController().signal,
          AbortSignal.timeout(total),
        ]);
        signal.throwIfAborted();
        const handshake = new URL(target.href);
        handshake.protocol = target.protocol === 'wss:' ? 'https:' : 'http:';
        const dispatcher = dispatchFor(handshake, signal),
          socket = new WebSocket(target.href, {
            protocols: init.protocols,
            headers: init.headers as never,
            dispatcher,
          });
        sockets.add(socket);
        const abort = () => socket.close(1001, 'cancelled');
        signal.addEventListener('abort', abort, { once: true });
        socket.addEventListener(
          'close',
          () => {
            sockets.delete(socket);
            signal.removeEventListener('abort', abort);
          },
          { once: true },
        );
        let received = 0;
        socket.addEventListener('message', (event) => {
          const data = event.data;
          received +=
            typeof data === 'string'
              ? Buffer.byteLength(data)
              : data instanceof Blob
                ? data.size
                : data instanceof ArrayBuffer
                  ? data.byteLength
                  : ArrayBuffer.isView(data)
                    ? data.byteLength
                    : 0;
          if (received > profile.maxResponseBytes) {
            event.stopImmediatePropagation();
            socket.close(1009, 'response limit');
            options.onDiagnostic?.({
              resourceRef: resource.resourceRef,
              profileId,
              phase: 'body',
              bytesRead: received,
              problem: problem(
                'network.response_too_large',
                'resource_pressure',
                'WebSocket接收超过字节上限',
              ).problem,
            });
          }
        });
        registered.reportApplied(snapshot);
        return socket;
      },
      async close() {
        if (clientClosed) return;
        clientClosed = true;
        ownerAbort.abort();
        for (const socket of sockets) socket.close(1001, 'client closed');
        sockets.clear();
        pool.users--;
        if (pool.retired && pool.users === 0) pool.done.resolve();
        leases.delete(result);
      },
    };
    leases.add(result);
    return result;
  }
  return {
    client,
    credentialTarget,
    async withClient<T>(
      ctx: CallContext,
      selection: NetworkSelection,
      use: (client: NetworkClient) => Promise<T>,
    ): Promise<T> {
      const value = await client(ctx, selection);
      try {
        return await use(value);
      } finally {
        await value.close();
      }
    },
    async inspect(
      ctx: CallContext,
      input: { scope?: ConfigScope; profileId?: string } = {},
    ) {
      const scope = input.scope ?? {
        kind: 'installation' as const,
        installationId: options.installationId,
      };
      if (scope.kind !== 'installation' && ctx.subjectId !== scope.subjectId)
        throw problem(
          'network.scope_mismatch',
          'forbidden',
          '网络配置范围与当前主体不符',
        );
      await options.authorize(ctx, 'network.read', {
        product: options.product,
        installationId: options.installationId,
        ...(scope.kind === 'installation'
          ? {}
          : { subjectId: scope.subjectId }),
        kind: 'NetworkProfile',
        id: input.profileId ?? 'default',
      });
      const snapshot = await consumer(scope).snapshot(),
        value = NetworkSettingsSchema.parse(snapshot.values);
      return {
        defaultProfileId: value.defaultProfileId,
        profiles: Object.entries(value.profiles)
          .filter(([id]) => !input.profileId || id === input.profileId)
          .map(([id, p]) => ({
            id,
            proxy: p.proxy.mode,
            tlsVerified: p.tls.rejectUnauthorized,
          })),
        revisionRefs: snapshot.revisions,
        pools: pools.size,
        activeClients: leases.size,
      };
    },
    async probe(
      ctx: CallContext,
      input: NetworkSelection & { kind: 'dns' | 'tls' | 'http'; url: string },
    ) {
      const resource = ResourceNetworkScopeSchema.parse(input.resource),
        url = authorizedUrl(
          resource,
          requestUrl(input.url),
          input.kind === 'dns' ? resource.methods[0] : 'HEAD',
        );
      await options.authorize(ctx, 'network.probe', resource.resourceRef);
      if (input.kind === 'dns') {
        const values = await lookup(url.hostname.replace(/^\[|\]$/g, ''), {
          all: true,
        });
        return {
          kind: 'dns',
          resolution: 'local',
          addresses: values.map((value) => ({
            ...value,
            allowed: allowedAddress(resource, value.address),
          })),
        };
      }
      if (input.kind === 'tls' && url.protocol !== 'https:')
        throw problem(
          'network.invalid_probe',
          'validation',
          'TLS探测需要HTTPS地址',
        );
      return this.withClient(ctx, input, async (value) => {
        const response = await value.fetch(url, { method: 'HEAD' });
        await response.body?.cancel();
        return { kind: input.kind, status: response.status, url: response.url };
      });
    },
    async closeScope(scope: ConfigScope) {
      const key = canonicalize(scope);
      for (const value of [...leases])
        if (includesScope(scope, value.metadata.scope)) await value.close();
      for (const [id, value] of pools)
        if (includesScope(scope, JSON.parse(value.scopeKey))) pools.delete(id);
      for (const [id, value] of consumers)
        if (includesScope(scope, JSON.parse(id))) {
          value.close();
          consumers.delete(id);
        }
    },
    async closeResource(ref: Ref) {
      for (const value of [...leases])
        if (objectKey(value.metadata.resourceRef) === objectKey(ref))
          await value.close();
      for (const [key, value] of pools)
        if (objectKey(value.resourceRef) === objectKey(ref)) pools.delete(key);
    },
    async close() {
      if (closed) return;
      closed = true;
      unsubscribe();
      await Promise.all([...leases].map((value) => value.close()));
      pools.clear();
      for (const value of consumers.values()) value.close();
      consumers.clear();
      await Promise.allSettled([...pending]);
    },
  };
}
export interface NetworkClient {
  fetch: typeof fetch;
  metadata: NetworkMetadata;
  webSocket(
    url: string,
    options?: {
      protocols?: string | string[];
      headers?: HeadersInit;
      signal?: AbortSignal;
    },
  ): Promise<WebSocket>;
  close(): Promise<void>;
}
export type Network = ReturnType<typeof createNetwork>;
