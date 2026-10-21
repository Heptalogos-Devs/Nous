import type { EnvironmentProxySnapshot } from './environment.js';
import {
  Agent,
  EnvHttpProxyAgent,
  ProxyAgent,
  Socks5ProxyAgent,
  type Dispatcher,
} from 'undici';
import { getCACertificates } from 'node:tls';
import { z } from 'zod';
import { problem } from '../errors.js';
import { guardedLookup } from './address.js';
import type {
  NetworkOptions,
  NetworkProfile,
  ResourceNetworkScope,
} from './contracts.js';
import type { CallContext, Ref } from '@nous-wave/contracts/common';
import type { SecretUseMetadata } from '../credentials/secrets.js';
export type Material = { bytes: Uint8Array; metadata: SecretUseMetadata };
export type Materials = Map<string, Material>;
export function materialRefs(
  profile: NetworkProfile,
): { ref: string; purpose: string }[] {
  return [
    ...profile.tls.caFileRefs.map((ref) => ({ ref, purpose: 'network.ca' })),
    ...(profile.tls.clientCertificateRef
      ? [
          {
            ref: profile.tls.clientCertificateRef,
            purpose: 'network.client_certificate',
          },
        ]
      : []),
    ...(profile.tls.clientKeyRef
      ? [{ ref: profile.tls.clientKeyRef, purpose: 'network.client_key' }]
      : []),
    ...('credentialRef' in profile.proxy && profile.proxy.credentialRef
      ? [{ ref: profile.proxy.credentialRef, purpose: 'network.proxy' }]
      : []),
  ];
}
export function withMaterials<T>(
  options: NetworkOptions,
  ctx: CallContext,
  resource: ResourceNetworkScope,
  profileRef: Ref,
  profile: NetworkProfile,
  use: (materials: Materials) => Promise<T>,
): Promise<T> {
  const refs = materialRefs(profile),
    values: Materials = new Map();
  async function next(index: number): Promise<T> {
    if (index === refs.length) return use(values);
    if (!options.withMaterial)
      throw problem(
        'network.material_unavailable',
        'unavailable',
        '网络TLS或代理材料解析器未配置',
      );
    const item = refs[index];
    return options.withMaterial(
      ctx,
      { ...item, resourceRef: profileRef, useRef: resource.resourceRef },
      async (bytes, metadata) => {
        values.set(item.ref, { bytes, metadata });
        try {
          return await next(index + 1);
        } finally {
          values.delete(item.ref);
        }
      },
    );
  }
  return next(0);
}
const proxyCredential = z.strictObject({
  username: z.string().max(1024),
  password: z.string().max(4096),
});
export function createTransport(
  profile: NetworkProfile,
  resource: ResourceNetworkScope,
  materials: Materials,
  environment?: EnvironmentProxySnapshot,
): Dispatcher {
  const bytes = (ref: string | undefined) => {
    if (!ref) return undefined;
    const value = materials.get(ref);
    if (!value)
      throw problem(
        'network.material_unavailable',
        'unavailable',
        '网络材料当前不可用',
      );
    return Buffer.from(
      value.bytes.buffer,
      value.bytes.byteOffset,
      value.bytes.byteLength,
    );
  };
  const ca: (string | Buffer)[] = profile.tls.useSystemCA
    ? [
        ...new Set([
          ...getCACertificates('default'),
          ...getCACertificates('system'),
        ]),
      ]
    : [];
  for (const ref of profile.tls.caFileRefs) ca.push(bytes(ref)!);
  const tls = {
    ca,
    cert: bytes(profile.tls.clientCertificateRef),
    key: bytes(profile.tls.clientKeyRef),
    rejectUnauthorized: profile.tls.rejectUnauthorized,
    timeout: profile.timeout.connectMs,
  };
  const common = {
    connections: profile.connections,
    pipelining: profile.pipelining,
    maxHeaderSize: profile.maxHeaderBytes,
    headersTimeout: profile.timeout.headersMs,
    bodyTimeout: profile.timeout.bodyIdleMs,
    connect: { ...tls, lookup: guardedLookup(resource) },
  };
  let credential: z.infer<typeof proxyCredential> | undefined;
  if ('credentialRef' in profile.proxy && profile.proxy.credentialRef) {
    try {
      credential = proxyCredential.parse(
        JSON.parse(
          new TextDecoder('utf-8', { fatal: true }).decode(
            bytes(profile.proxy.credentialRef),
          ),
        ),
      );
    } catch {
      throw problem(
        'network.proxy_credentials_invalid',
        'unavailable',
        '代理凭据必须提供username和password',
      );
    }
  }
  const proxy = {
    ...common,
    requestTls: tls,
    proxyTls: {
      ca,
      rejectUnauthorized: profile.tls.rejectUnauthorized,
      timeout: profile.timeout.connectMs,
    },
    connectTimeout: profile.timeout.connectMs,
  };
  switch (profile.proxy.mode) {
    case 'direct':
      return new Agent(common);
    case 'environment':
      return new EnvHttpProxyAgent({
        ...proxy,
        ...environment,
      });
    case 'http':
      return new ProxyAgent({
        ...proxy,
        uri: profile.proxy.url,
        ...(credential
          ? {
              token: `Basic ${Buffer.from(`${credential.username}:${credential.password}`).toString('base64')}`,
            }
          : {}),
      });
    case 'socks5': {
      const { connect, proxyTls, ...socketOptions } = proxy;
      return new Socks5ProxyAgent(profile.proxy.url, {
        ...socketOptions,
        ...credential,
      });
    }
  }
}
