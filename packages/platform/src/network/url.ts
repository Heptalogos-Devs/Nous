// Credential-free URL validation adapted from Heptalogos 21f0080460f3ee639deb9f5ea47282fb6c59df95
// packages/system/network-access/src/service.ts; Copyright 2026 Aravine Zhu.
// SPDX-License-Identifier: AGPL-3.0-only
import { problem } from '../errors.js';
import type { ResourceNetworkScope } from './contracts.js';
export function requestUrl(input: Parameters<typeof fetch>[0] | URL): URL {
  try {
    return new URL(
      input instanceof URL
        ? input.href
        : typeof input === 'string'
          ? input
          : input.url,
    );
  } catch {
    throw problem('network.invalid_url', 'validation', '网络地址无效');
  }
}
function path(value: string): string {
  let decoded: string;
  try {
    decoded = decodeURIComponent(value);
  } catch {
    throw problem('network.invalid_url', 'validation', '网络路径编码无效');
  }
  if (
    decoded.includes('\\') ||
    decoded.includes('\0') ||
    /%(?:2e|2f|5c)/i.test(decoded)
  )
    throw problem(
      'network.unauthorized_target',
      'forbidden',
      '网络路径超出资源范围',
    );
  return new URL(decoded, 'http://scope.invalid').pathname;
}
export function authorizedUrl(
  resource: ResourceNetworkScope,
  input: URL,
  method: string,
): URL {
  const url = new URL(input.href);
  const protocol =
    url.protocol === 'ws:'
      ? 'http:'
      : url.protocol === 'wss:'
        ? 'https:'
        : url.protocol;
  const origin = protocol + '//' + url.host;
  if (
    !['http:', 'https:'].includes(protocol) ||
    url.username ||
    url.password ||
    url.hash ||
    !resource.allowedOrigins.includes(origin) ||
    !resource.methods.includes(method.toUpperCase())
  )
    throw problem(
      'network.unauthorized_target',
      'forbidden',
      '网络目标或方法超出资源范围',
    );
  const target = path(url.pathname);
  if (
    !resource.allowedPathPrefixes.some((prefix) => {
      const base = path(prefix);
      return (
        base === '/' ||
        target === base ||
        target.startsWith(base.replace(/\/$/, '') + '/')
      );
    })
  )
    throw problem(
      'network.unauthorized_target',
      'forbidden',
      '网络路径超出资源范围',
    );
  return url;
}
export function contentLength(headers: Headers): number | undefined {
  const value = headers.get('content-length');
  if (!value || !/^\d+$/.test(value)) return undefined;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : undefined;
}
