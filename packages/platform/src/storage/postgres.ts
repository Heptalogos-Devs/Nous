import { Temporal } from '@js-temporal/polyfill';
import { Client, types as pgTypes } from 'pg';
import type { ClientConfig, CustomTypesConfig } from 'pg';
import { parse as parseArray } from 'postgres-array';
import { ProblemError, problem } from '../errors.js';

type PgProblem = ProblemError & { code?: string; constraint?: string };
type PgOperation = 'connect' | 'query' | 'migration' | 'close';

/** PostgreSQL errors must be safe before the datasource checkpoints them. */
export function postgresProblem(error: unknown, operation: PgOperation): PgProblem {
  if (error instanceof ProblemError) return error;
  const original = typeof error === 'object' && error !== null
    ? error as Record<string, unknown> : {};
  const code = typeof original.code === 'string' && /^[A-Z0-9_]{2,32}$/.test(original.code)
    ? original.code : undefined;
  const sqlState = code?.length === 5 ? code : undefined;
  const constraint = typeof original.constraint === 'string'
    && /^[a-zA-Z_][a-zA-Z0-9_]{0,62}$/.test(original.constraint)
    ? original.constraint : undefined;
  const category = operation === 'connect' || code?.startsWith('08')
    || code?.startsWith('57') || ['ECONNREFUSED', 'ECONNRESET', 'ETIMEDOUT', 'ENOTFOUND', 'EPIPE'].includes(code ?? '')
    ? 'unavailable'
    : code?.startsWith('53') ? 'resource_pressure'
    : code === '42501' ? 'forbidden'
    : ['40001', '40P01', '23505', '23503'].includes(code ?? '') ? 'conflict'
    : code?.startsWith('22') || ['23502', '23514'].includes(code ?? '') ? 'validation'
    : 'failed';
  const safe = problem(
    `storage.${operation}_failed`, category,
    operation === 'connect' ? '数据库连接不可用' : operation === 'migration' ? '数据库迁移失败' : operation === 'close' ? '数据库关闭失败' : '数据库查询失败',
    { details: { ...(sqlState ? { sqlState } : {}), ...(constraint ? { constraint } : {}) } },
  ) as PgProblem;
  // DBOS 5.3 reads .code before deciding whether to retry a transaction.
  if (code) Object.defineProperty(safe, 'code', { value: code, enumerable: true });
  if (constraint) safe.constraint = constraint;
  return safe;
}

function precisionProblem(): ProblemError {
  return problem('storage.precision_invalid', 'validation', '数据库数值超出精度合同；精确大数应保存为十进制字符串');
}

function timestamp(value: string): string {
  try {
    const instant = Temporal.Instant.from(value);
    const year = instant.toZonedDateTimeISO('UTC').year;
    if (year < 1 || year > 9999 || instant.epochNanoseconds % 1000n !== 0n) throw precisionProblem();
    return instant.toString({ fractionalSecondDigits: 6 });
  } catch {
    throw precisionProblem();
  }
}

function json(value: string): unknown {
  return JSON.parse(value, (_key, current: unknown) => {
    if (typeof current === 'number' && (!Number.isFinite(current)
      || (Number.isInteger(current) && !Number.isSafeInteger(current)))) throw precisionProblem();
    return current;
  });
}

const textParsers = new Map<number, (value: string) => unknown>([
  [20, value => value], [1700, value => value],
  [1082, value => value], [1114, value => value], [1184, timestamp],
  [114, json], [3802, json],
]);
for (const [arrayOid, itemOid] of [[1016, 20], [1231, 1700], [1182, 1082], [1115, 1114], [1185, 1184], [199, 114], [3807, 3802]]) {
  textParsers.set(arrayOid, value => parseArray(value, textParsers.get(itemOid)!));
}

/** Instance-local codecs; never mutate pg's global type registry. */
export function postgresTypes(base: CustomTypesConfig = pgTypes): CustomTypesConfig {
  return {
    getTypeParser: ((oid: number, format: 'text' | 'binary' = 'text') => {
      const parser = textParsers.get(oid);
      if (!parser) return base.getTypeParser(oid, format as 'text');
      if (format === 'text') return parser;
      // The business contract uses text protocol for exact timestamps/numerics.
      return () => { throw problem('storage.binary_precision_unsupported', 'validation', '精确数据库类型请使用文本传输格式'); };
    }) as typeof pgTypes.getTypeParser,
  };
}

/** One driver boundary covers Kysely, datasource and the shared auth Pool. */
export class StorageClient extends Client {
  constructor(config?: string | ClientConfig) {
    try { super(config); } catch (error) { throw postgresProblem(error, 'connect'); }
  }

  override connect: Client['connect'] = ((callback?: (error?: Error) => void) => {
    if (callback) return super.connect(error => callback(error ? postgresProblem(error, 'connect') : undefined));
    return super.connect().catch(error => { throw postgresProblem(error, 'connect'); });
  }) as Client['connect'];

  override emit(event: string | symbol, ...args: unknown[]): boolean {
    if (event === 'error') args[0] = postgresProblem(args[0], 'connect');
    return super.emit(event, ...args);
  }

  override query: Client['query'] = ((...args: unknown[]) => {
    const last = args.at(-1);
    if (typeof last === 'function') {
      args[args.length - 1] = (error: unknown, result: unknown) => last(error ? postgresProblem(error, 'query') : null, result);
    }
    try {
      const result = (super.query as (...values: unknown[]) => unknown).apply(this, args);
      if (result && typeof (result as Promise<unknown>).catch === 'function') {
        return (result as Promise<unknown>).catch(error => { throw postgresProblem(error, 'query'); });
      }
      return result;
    } catch (error) { throw postgresProblem(error, 'query'); }
  }) as Client['query'];
}
