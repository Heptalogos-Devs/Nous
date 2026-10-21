import { ProblemError, problem } from '../errors.js';
export function networkError(
  error: unknown,
  phase: string,
  signal?: AbortSignal,
): ProblemError {
  if (error instanceof ProblemError) return error;
  let value = error;
  let code: string | undefined;
  for (let i = 0; i < 5 && value && typeof value === 'object'; i++) {
    if (value instanceof ProblemError) return value;
    if ('code' in value && typeof value.code === 'string') code = value.code;
    value = 'cause' in value ? value.cause : undefined;
  }
  const category = signal?.aborted
    ? signal.reason instanceof DOMException &&
      signal.reason.name === 'TimeoutError'
      ? 'timeout'
      : 'cancelled'
    : ['ENOTFOUND', 'EAI_AGAIN', 'ENODATA'].includes(code ?? '')
      ? 'dns_failed'
      : code === 'UND_ERR_CONNECT_TIMEOUT'
        ? 'connect_timeout'
        : code?.startsWith('ERR_TLS_') ||
            code?.includes('CERT') ||
            code === 'DEPTH_ZERO_SELF_SIGNED_CERT'
          ? 'tls_failed'
          : code === 'UND_ERR_HEADERS_TIMEOUT' ||
              code === 'UND_ERR_BODY_TIMEOUT'
            ? 'timeout'
            : 'transport_failed';
  const messages: Record<string, string> = {
    timeout: '网络请求超时',
    cancelled: '网络请求已取消',
    dns_failed: '网络地址解析失败',
    connect_timeout: '网络连接超时',
    tls_failed: '网络TLS验证失败',
    transport_failed: '网络传输失败',
  };
  return problem(`network.${category}`, 'unavailable', messages[category], {
    details: { phase, ...(code ? { transportCode: code } : {}) },
  });
}
