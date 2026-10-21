import { ProblemError, ProblemSchema } from './common.js';

function responseError(value: unknown, status: number): Error {
  const parsed = ProblemSchema.safeParse(value);
  return parsed.success
    ? new ProblemError(parsed.data)
    : new ProblemError({
        code: 'http.invalid_response',
        category: 'unavailable',
        status: status >= 400 && status <= 599 ? status : 502,
        messageKey: 'http.invalid_response',
        message: '服务返回了无法读取的错误响应',
      });
}
export const problemMiddleware = {
  async post({ response }: { response: Response }): Promise<Response> {
    if (!response.ok)
      throw responseError(
        await response
          .clone()
          .json()
          .catch(() => undefined),
        response.status,
      );
    return response;
  },
};

/** Request metadata for the generated client; identity is resolved by the server. */
export interface ContextHeadersOptions {
  submissionId?: string;
  traceparent?: string;
  purpose?: string;
  audience?: readonly string[];
  delegationAuthority?: string;
}
export function contextHeaders(
  options: ContextHeadersOptions,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries({
      'idempotency-key': options.submissionId,
      traceparent: options.traceparent,
      'x-purpose': options.purpose,
      'x-audience':
        options.audience === undefined
          ? undefined
          : JSON.stringify(options.audience),
      'x-delegation-authority': options.delegationAuthority,
    }).filter((entry): entry is [string, string] => entry[1] !== undefined),
  );
}
