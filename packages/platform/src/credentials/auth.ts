// Better Auth owns password, session and API-key mechanics. Product scope is
// resolved by the current delegation registry, never API-key metadata.
import { betterAuth } from 'better-auth';
import { isAPIError } from 'better-auth/api';
import { admin } from 'better-auth/plugins';
import { apiKey } from '@better-auth/api-key';
import { getMigrations } from 'better-auth/db/migration';
import { getAdapter } from 'better-auth/db/adapter';
import {
  ProblemError,
  type Consumer,
  type CallContext,
} from '@nous-wave/contracts/common';
import { sql, type Storage, type Kysely } from '../storage/index.js';
import { problem } from '../errors.js';
import { parseInstant } from '../time/index.js';
import { createDelegations, type DelegationOptions } from './delegations.js';

export interface AuthenticationOptions
  extends Pick<
    DelegationOptions,
    | 'product'
    | 'installationId'
    | 'operationPolicy'
    | 'sourceGuard'
    | 'issuancePolicy'
  > {
  pool: Storage<any>['pool'];
  db: Storage<any>['db'];
  secret: string;
  baseURL: string;
  trustedOrigins: string[];
}
export async function createAuthentication(options: AuthenticationOptions) {
  const keyPlugin = apiKey({
    enableMetadata: false,
    maximumNameLength: 256,
    keyExpiration: { minExpiresIn: 1 / 86400, maxExpiresIn: 365 },
  });
  const config = {
    database: options.pool,
    secret: options.secret,
    baseURL: options.baseURL,
    basePath: '/v1/auth',
    trustedOrigins: options.trustedOrigins,
    advanced: {
      cookiePrefix: options.product === 'hept' ? 'heptalogos' : 'nous-wave',
    },
    session: { cookieCache: { enabled: false } },
    emailAndPassword: { enabled: true, disableSignUp: true },
    plugins: [admin(), keyPlugin],
    logger: { disabled: true },
  };
  await (await getMigrations(config)).runMigrations();
  const auth = betterAuth(config);
  // Cache adapter objects by the explicitly supplied connection, never authorization results.
  const adapters = new WeakMap<Kysely<any>, ReturnType<typeof getAdapter>>();
  async function adapter(database?: Kysely<any>) {
    const ctx = await auth.$context;
    if (!database) return ctx.adapter;
    let current = adapters.get(database);
    if (!current) {
      // Product CamelCasePlugin is not the casing of Better Auth's native schema.
      current = getAdapter({
        ...ctx.options,
        database: {
          db: database.withoutPlugins(),
          type: 'postgres',
          transaction: false,
          debugLogs: false,
        },
      });
      adapters.set(database, current);
      void current.catch(() => adapters.delete(database));
    }
    return current;
  }
  const user = async (id: string, database?: Kysely<any>) => {
    const record = await (await adapter(database)).findOne<{
      id: string;
      role?: string;
      banned?: boolean;
      banExpires?: Date | string | null;
    }>({ model: 'user', where: [{ field: 'id', value: id }] });
    if (!record) return null;
    return {
      id: record.id,
      admin: record.role?.split(',').includes('admin') ?? false,
      enabled:
        !record.banned ||
        Boolean(
          record.banExpires &&
            new Date(record.banExpires).getTime() <= Date.now(),
        ),
    };
  };
  const registry = createDelegations({
    ...options,
    currentUser: user,
    async currentKey(id, database) {
      const key = await (await adapter(database)).findOne<{
        id: string;
        referenceId: string;
        enabled: boolean;
        expiresAt?: Date | string | null;
      }>({ model: 'apikey', where: [{ field: 'id', value: id }] });
      return key
        ? {
            userId: key.referenceId,
            enabled: key.enabled,
            expiresAt: key.expiresAt
              ? parseInstant(
                  typeof key.expiresAt === 'string'
                    ? key.expiresAt
                    : key.expiresAt.toISOString(),
                )
              : undefined,
          }
        : null;
    },
    async currentSession(id, database) {
      const session = await (await adapter(database)).findOne<{
        userId: string;
        expiresAt: Date | string;
      }>({ model: 'session', where: [{ field: 'id', value: id }] });
      return session
        ? {
            userId: session.userId,
            expiresAt: parseInstant(
              typeof session.expiresAt === 'string'
                ? session.expiresAt
                : session.expiresAt.toISOString(),
            ),
          }
        : null;
    },
    async createKey(userId, input, database) {
      try {
        const body = {
          userId,
          name: input.name,
          expiresIn: input.expiresIn,
          rateLimitEnabled: true,
          rateLimitMax: 600,
          rateLimitTimeWindow: 60000,
        };
        let transactional: Awaited<typeof auth.$context> | undefined;
        if (database) {
          transactional = {
            ...(await auth.$context),
            adapter: await adapter(database),
          };
          await keyPlugin.endpoints.deleteAllExpiredApiKeys({
            context: transactional,
          });
        }
        const key = transactional
          ? await keyPlugin.endpoints.createApiKey({
              body,
              context: transactional,
            })
          : await auth.api.createApiKey({
              body: {
                userId,
                name: input.name,
                expiresIn: input.expiresIn,
                rateLimitEnabled: true,
                rateLimitMax: 600,
                rateLimitTimeWindow: 60000,
              },
            });
        return { id: key.id, key: key.key };
      } catch (error) {
        if (isAPIError(error) && error.statusCode === 400)
          throw problem(
            'credentials.invalid_key_input',
            'validation',
            '凭据名称或有效期无效',
          );
        throw error;
      }
    },
    async disableKey(id, database) {
      await (await adapter(database)).update({
        model: 'apikey',
        where: [{ field: 'id', value: id }],
        update: { enabled: false },
      });
    },
  });
  const consumer: Consumer = {
    kind: 'maintenance',
    host: options.product,
    id: options.installationId,
  };
  return {
    handler: async (request: Request) => auth.handler(request),
    async authenticate(headers: Headers) {
      const raw = headers.get('authorization');
      const key =
        raw?.match(/^Bearer\s+(.+)$/i)?.[1] ?? headers.get('x-api-key');
      if (raw && !key)
        throw problem(
          'credentials.invalid_credential',
          'forbidden',
          '认证凭据无效',
        );
      try {
        if (key) {
          const result = await auth.api.verifyApiKey({ body: { key } });
          if (!result.valid || !result.key)
            throw problem(
              'credentials.invalid_credential',
              'forbidden',
              '认证凭据无效',
            );
          return registry.authenticateKey(result.key.id);
        }
        const session = await auth.api.getSession({
          headers,
          query: { disableCookieCache: true, disableRefresh: true },
        });
        if (!session || !(await user(session.user.id))?.enabled)
          throw problem('credentials.login_required', 'forbidden', '请先登录');
        return {
          actor: {
            id: session.user.id,
            kind: 'human' as const,
            authorityRef: `session:${session.session.id}`,
          },
          consumer,
        };
      } catch (error) {
        if (error instanceof ProblemError) throw error;
        throw problem(
          'credentials.invalid_credential',
          'forbidden',
          '认证凭据无效',
        );
      }
    },
    selectDelegation: registry.selectDelegation,
    authorize: (
      ctx: CallContext,
      operation: string,
      target?: Parameters<typeof registry.authorize>[2],
      database?: Kysely<any>,
    ) => registry.authorize(ctx, operation, target, database),
    async bootstrapAdministrator(input: {
      email: string;
      password: string;
      name: string;
    }) {
      // Keep the native API and its transactions on the connection holding the claim lock.
      return options.db.connection().execute(async (connection) => {
        try {
          await sql`select pg_advisory_lock(1734512829)`.execute(connection);
          const existing = await (await adapter(connection)).findMany({
            model: 'user',
            limit: 1,
          });
          if (existing.length)
            throw problem(
              'credentials.already_initialized',
              'conflict',
              '安装已经建立管理员',
            );
          const native = betterAuth({
            ...config,
            database: {
              db: connection.withoutPlugins(),
              type: 'postgres',
              transaction: true,
            },
          });
          const created = await native.api.createUser({
            body: { ...input, role: 'admin' },
          });
          return { userId: created.user.id };
        } catch (error) {
          if (error instanceof ProblemError) throw error;
          throw problem(
            'credentials.bootstrap_failed',
            'unavailable',
            '管理员初始化失败',
          );
        } finally {
          await sql`select pg_advisory_unlock(1734512829)`.execute(connection);
        }
      });
    },
    /** Local Bootstrap/Recovery composition only; never mounted on the product HTTP router. */
    recovery: {
      async resetAdministrator(input: {
        email: string;
        password: string;
        unban?: boolean;
      }) {
        const configured = await auth.$context;
        if (
          input.password.length <
            configured.password.config.minPasswordLength ||
          input.password.length > configured.password.config.maxPasswordLength
        )
          throw problem(
            'credentials.password_length',
            'validation',
            '新密码长度不符合本安装的密码规则',
          );
        const hashedPassword = await configured.password.hash(input.password);
        return options.db.transaction().execute(async (transaction) => {
          const recovery = await betterAuth({
            ...config,
            database: {
              db: transaction.withoutPlugins(),
              type: 'postgres',
              transaction: false,
            },
          }).$context;
          const found = await recovery.internalAdapter.findUserByEmail(
            input.email,
          );
          const account = found?.user;
          if (!account || !(await user(account.id, transaction))?.admin)
            throw problem(
              'credentials.administrator_not_found',
              'not_found',
              '选定管理员不存在',
            );
          // Same native owner operations as Better Auth admin set-user-password.
          if (await recovery.internalAdapter.findCredentialAccount(account.id))
            await recovery.internalAdapter.updatePassword(
              account.id,
              hashedPassword,
            );
          else
            await recovery.internalAdapter.createAccount({
              userId: account.id,
              providerId: 'credential',
              accountId: account.id,
              password: hashedPassword,
            });
          await recovery.internalAdapter.deleteUserSessions(account.id);
          if (input.unban)
            await recovery.internalAdapter.updateUser(account.id, {
              banned: false,
              banReason: null,
              banExpires: null,
            });
          return {
            userId: account.id,
            passwordReset: true,
            sessionsRevoked: true,
            unbanned: input.unban === true,
          };
        });
      },
    },
    currentAuthority: registry.currentAuthority,
    createManagementKey: registry.createManagementKey,
    createRuntimeKey: registry.createRuntimeKey,
    deleteSubject: registry.deleteSubject,
    delegations: {
      create: registry.create,
      createChild: registry.createChild,
      read: registry.read,
      update: registry.update,
      revoke: registry.revoke,
    },
  };
}
export type Authentication = Awaited<ReturnType<typeof createAuthentication>>;
