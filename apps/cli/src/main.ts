#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Configuration,
  DefaultApi,
  problemMiddleware,
} from '@nous-wave/client';
import {
  createBootstrap,
  createPostgresRuntime,
  createServiceManager,
  postgresRuntimeRef,
} from '@nous-wave/platform/bootstrap';
import { createBootstrapSecrets } from '@nous-wave/platform/credentials';
import { problem, safeProblem } from '@nous-wave/platform/errors';
import { readReal } from '@nous-wave/platform/time';
import { Command } from 'commander';
import { addManagementCommands } from './management.js';

const cli = new Command('nous')
  .description('Nous Wave 长期认知服务与安装管理')
  .option('--config <path>', '实例引导文件')
  .option('--instance <path>', '实例目录')
  .option('--url <url>', '已运行服务地址')
  .option('--json', '输出结构化结果')
  .option('--submission-id <id>', '本次提交的稳定身份；重试沿用同一值');
const programRoot = dirname(
  fileURLToPath(import.meta.resolve('@nous-wave/nous/package.json')),
);
const decoder = new TextDecoder('utf-8', { fatal: true });
function show(value: unknown) {
  process.stdout.write(JSON.stringify(value, null, 2) + '\n');
}
function locator() {
  const options = cli.opts();
  return createBootstrap({
    product: 'nous',
    programRoot,
    configPath: options.config,
    instanceHome: options.instance,
  });
}
async function withClient<T>(
  use: (client: DefaultApi) => Promise<T>,
): Promise<T> {
  const view = await locator().inspect();
  const { listen } = view.bootstrap;
  const baseURL =
    cli.opts().url ??
    listen.publicBaseURL ??
    `http://${listen.host}:${listen.port}`;
  return createBootstrapSecrets(view.paths.secret).withSecret(
    'cli-auth',
    { purpose: 'cli.authentication' },
    (bytes) =>
      use(
        new DefaultApi(
          new Configuration({
            basePath: baseURL,
            accessToken: decoder.decode(bytes),
            middleware: [problemMiddleware],
            ...(cli.opts().submissionId
              ? { headers: { 'idempotency-key': cli.opts().submissionId } }
              : {}),
          }),
        ),
      ),
  );
}
function fromEnvironment(variable: string): Buffer {
  const value = process.env[variable];
  if (!value)
    throw problem(
      'credentials.environment_missing',
      'unavailable',
      '指定的凭据环境变量不可用',
    );
  return Buffer.from(value);
}

cli
  .command('init')
  .description('创建空安装；不启动主体或模型')
  .option('--database-mode <mode>', 'external或managed', 'external')
  .option('--postgres-port <port>', '本地PostgreSQL端口', Number, 55433)
  .option(
    '--database-url-env <name>',
    '读取数据库连接凭据的环境变量',
    'NOUS_DATABASE_URL',
  )
  .option(
    '--credential-backend <backend>',
    '数据库凭据后端：file 或 environment',
    'file',
  )
  .option('--host <host>', '监听地址', '127.0.0.1')
  .option('--port <port>', '监听端口', Number, 4333)
  .action(async (options) => {
    if (!['file', 'environment'].includes(options.credentialBackend))
      throw problem(
        'bootstrap.invalid_backend',
        'validation',
        '请选择file或environment凭据后端',
      );
    if (!['external', 'managed'].includes(options.databaseMode))
      throw problem(
        'bootstrap.invalid_database_mode',
        'validation',
        '请选择external或managed数据库',
      );
    const view = await locator().init({
      ...(options.databaseMode === 'managed'
        ? {
            database: {
              mode: 'managed',
              runtimeRef: postgresRuntimeRef(),
              port: options.postgresPort,
              database: 'nous_wave',
              user: 'nous',
              passwordRef: 'database-password',
              maxConnections: 10,
            },
          }
        : {}),
      listen: {
        host: options.host,
        port: options.port,
        publicBaseURL: `http://${options.host}:${options.port}`,
        trustedOrigins: [`http://${options.host}:${options.port}`],
      },
    });
    const secrets = createBootstrapSecrets(view.paths.secret);
    const database = view.bootstrap.database;
    if (database.mode !== options.databaseMode)
      throw problem(
        'bootstrap.database_mode_conflict',
        'conflict',
        '该实例已配置其他数据库模式',
      );
    if (database.mode === 'managed')
      await secrets.ensureFile(database.passwordRef, {
        purpose: 'bootstrap.database',
      });
    if (
      database.mode === 'external' &&
      !(await secrets.status(database.urlRef)).configured
    ) {
      if (options.credentialBackend === 'environment')
        await secrets.bindEnv(database.urlRef, {
          variable: options.databaseUrlEnv,
          purpose: 'bootstrap.database',
        });
      else {
        const value = fromEnvironment(options.databaseUrlEnv);
        try {
          await secrets.setFile(database.urlRef, {
            value,
            purpose: 'bootstrap.database',
          });
        } finally {
          value.fill(0);
        }
      }
    }
    await secrets.ensureFile('auth-signing', { purpose: 'auth.signing' });
    show({
      installationId: view.bootstrap.installationId,
      configPath: view.configPath,
      paths: view.paths,
      initialized: true,
      subjectStarted: false,
      modelCalled: false,
    });
  });

const runtime = cli
  .command('runtime')
  .description('显式取得和检查可选原生运行资源');
for (const action of ['acquire', 'status'] as const)
  runtime.command(`${action} <kind>`).action(async (kind) => {
    if (kind !== 'postgres')
      throw problem(
        'bootstrap.runtime_unknown',
        'validation',
        '当前支持postgres运行资源',
      );
    const resource = createPostgresRuntime(await locator().inspect());
    show(
      await (action === 'acquire' ? resource.acquire() : resource.inspect()),
    );
  });

const bootstrap = cli
  .command('bootstrap')
  .description('离线查看和修改本安装引导');
bootstrap
  .command('inspect')
  .action(async () => show(await locator().inspect()));
bootstrap
  .command('configure <file>')
  .requiredOption('--expected-digest <digest>', '先前inspect的摘要')
  .action(async (file, options) =>
    show(
      await locator().configure({
        expectedDigest: options.expectedDigest,
        changes: JSON.parse(await readFile(file, 'utf8')),
      }),
    ),
  );
const bootstrapSecrets = bootstrap
  .command('secrets')
  .description('本机引导凭据；不打开业务数据库');
bootstrapSecrets.command('status <ref>').action(async (ref) => {
  const view = await locator().inspect();
  show(await createBootstrapSecrets(view.paths.secret).status(ref));
});
bootstrapSecrets
  .command('bind-env <ref> <variable>')
  .requiredOption('--purpose <purpose>')
  .action(async (ref, variable, options) => {
    const view = await locator().inspect();
    show(
      await createBootstrapSecrets(view.paths.secret).bindEnv(ref, {
        variable,
        purpose: options.purpose,
      }),
    );
  });
bootstrapSecrets
  .command('set <ref>')
  .requiredOption('--value-env <name>')
  .requiredOption('--purpose <purpose>')
  .action(async (ref, options) => {
    const view = await locator().inspect(),
      value = fromEnvironment(options.valueEnv);
    try {
      show(
        await createBootstrapSecrets(view.paths.secret).setFile(ref, {
          value,
          purpose: options.purpose,
        }),
      );
    } finally {
      value.fill(0);
    }
  });
bootstrapSecrets.command('revoke <ref>').action(async (ref) => {
  const view = await locator().inspect();
  show(await createBootstrapSecrets(view.paths.secret).revoke(ref));
});
cli
  .command('doctor')
  .description('离线检查引导、目录和必要依赖')
  .action(async () => {
    const result = await locator().doctor();
    let credentials: unknown, runtime: unknown;
    try {
      const view = await locator().inspect(),
        secrets = createBootstrapSecrets(view.paths.secret);
      const databaseRef =
        view.bootstrap.database.mode === 'external'
          ? view.bootstrap.database.urlRef
          : view.bootstrap.database.passwordRef;
      credentials = await Promise.all([
        secrets.status(databaseRef),
        secrets.status('auth-signing'),
      ]);
      if (view.bootstrap.database.mode === 'managed')
        runtime = await createPostgresRuntime(view).inspect();
    } catch (error) {
      credentials = { problem: safeProblem(error) };
    }
    show({ ...result, credentials, ...(runtime ? { runtime } : {}) });
  });

const service = cli
  .command('service')
  .description('系统原生服务管理')
  .option(
    '--scope <scope>',
    'user或system',
    process.platform === 'win32' ? 'system' : 'user',
  )
  .option('--account <account>', 'system模式实际运行账号')
  .option('--node <path>', '已安装Node绝对路径', process.execPath)
  .option(
    '--entrypoint <path>',
    '已编译Host入口',
    fileURLToPath(import.meta.resolve('@nous-wave/server/run')),
  )
  .option('--shawl <path>', 'Windows已安装Shawl绝对路径');
for (const action of [
  'plan',
  'install',
  'start',
  'stop',
  'status',
  'uninstall',
] as const) {
  service.command(action).action(async () => {
    const view = await locator().inspect(),
      options = service.opts();
    if (!['user', 'system'].includes(options.scope))
      throw problem(
        'service.invalid_scope',
        'validation',
        '服务范围须为user或system',
      );
    const manager = createServiceManager({
      view,
      nodePath: options.node,
      entrypoint: options.entrypoint ? resolve(options.entrypoint) : undefined,
      scope: options.scope,
      account: options.account,
      shawlPath: options.shawl,
    });
    show(await manager[action]());
  });
}

const recovery = cli
  .command('recovery')
  .description('本机安装所有者的限定恢复操作');
recovery
  .command('reset-administrator')
  .requiredOption('--email <email>')
  .option('--password-env <name>', '新密码环境变量', 'NOUS_ADMIN_PASSWORD')
  .option('--unban', '解除选定管理员的禁用')
  .action(async (options) => {
    const view = await locator().inspect(),
      bytes = fromEnvironment(options.passwordEnv);
    try {
      const { withNous } = await import('@nous-wave/nous');
      await withNous(
        view,
        async (app) =>
          show(
            await app.authentication.recovery.resetAdministrator({
              email: options.email,
              password: decoder.decode(bytes),
              unban: options.unban,
            }),
          ),
        { mode: 'cli' },
      );
    } finally {
      bytes.fill(0);
    }
  });

const auth = cli.command('auth').description('本机初始管理员与登录凭据');
auth
  .command('init')
  .requiredOption('--email <email>')
  .option('--name <name>', '显示名称', '管理员')
  .option('--password-env <name>', '管理员密码环境变量', 'NOUS_ADMIN_PASSWORD')
  .action(async (options) => {
    const view = await locator().inspect(),
      password = process.env[options.passwordEnv];
    if (!password)
      throw problem(
        'auth.password_missing',
        'validation',
        '请通过指定环境变量提供管理员密码',
      );
    const { withNous } = await import('@nous-wave/nous');
    await withNous(
      view,
      async (app) => {
        const authentication = app.authentication;
        const user = await authentication.bootstrapAdministrator({
          email: options.email,
          password,
          name: options.name,
        });
        const token = await authentication.createManagementKey(user.userId, {
          name: 'local-cli',
          scope: app.managementScope(),
        });
        const value = Buffer.from(token.key);
        try {
          await createBootstrapSecrets(view.paths.secret).setFile('cli-auth', {
            value,
            purpose: 'cli.authentication',
          });
        } finally {
          value.fill(0);
        }
        show({ created: true, userId: user.userId, credentialRef: 'cli-auth' });
      },
      { mode: 'cli' },
    );
  });
auth
  .command('login')
  .requiredOption('--email <email>')
  .option('--password-env <name>', '密码环境变量', 'NOUS_ADMIN_PASSWORD')
  .action(async (options) => {
    const view = await locator().inspect(),
      password = process.env[options.passwordEnv];
    if (!password)
      throw problem(
        'auth.password_missing',
        'validation',
        '请通过指定环境变量提供管理员密码',
      );
    const listen = view.bootstrap.listen;
    const baseURL =
      cli.opts().url ??
      listen.publicBaseURL ??
      `http://${listen.host}:${listen.port}`;
    const login = await fetch(new URL('/v1/auth/sign-in/email', baseURL), {
      method: 'POST',
      headers: { 'content-type': 'application/json', origin: baseURL },
      body: JSON.stringify({ email: options.email, password }),
    });
    if (!login.ok)
      throw problem('auth.login_failed', 'forbidden', '管理员登录失败');
    const cookie = login.headers
      .getSetCookie()
      .map((value) => value.split(';')[0])
      .join('; ');
    try {
      const response = await fetch(new URL('/v1/management-keys', baseURL), {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          cookie,
          origin: baseURL,
        },
        body: JSON.stringify({ name: 'local-cli' }),
      });
      if (!response.ok)
        throw problem('auth.key_failed', 'forbidden', '管理凭据签发失败');
      const token = (await response.json()) as { key: string };
      if (typeof token.key !== 'string')
        throw problem('auth.key_failed', 'unavailable', '管理凭据响应无效');
      const value = Buffer.from(token.key);
      try {
        await createBootstrapSecrets(view.paths.secret).setFile('cli-auth', {
          value,
          purpose: 'cli.authentication',
        });
      } finally {
        value.fill(0);
      }
      show({ authenticated: true, credentialRef: 'cli-auth' });
    } finally {
      await fetch(new URL('/v1/auth/sign-out', baseURL), {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          cookie,
          origin: baseURL,
        },
        body: '{}',
      });
    }
  });
cli
  .command('status')
  .description('读取已运行Host状态')
  .action(async () => show(await withClient((client) => client.hostStatus())));

const subjects = cli.command('subjects').description('主体管理');
subjects
  .command('create <name>')
  .option('--full', '启用完整认知')
  .action(async (name, options) =>
    show(
      await withClient((client) =>
        client.subjectsCreate({
          subjectCreateInput: {
            displayName: name,
            capabilities: options.full
              ? [
                  'Memory',
                  'Episode',
                  'Journal',
                  'Schema',
                  'Concept',
                  'Self',
                  'Social',
                  'LanguageConvention',
                  'Motivation',
                ]
              : ['Memory'],
          },
        }),
      ),
    ),
  );
subjects
  .command('list')
  .action(async () =>
    show(await withClient((client) => client.subjectsList())),
  );
subjects
  .command('read <subject>')
  .action(async (subject) =>
    show(
      await withClient((client) => client.subjectsRead({ subjectId: subject })),
    ),
  );
cli
  .command('query <subject> <text>')
  .description('认知查询')
  .action(async (subject, text) =>
    show(
      await withClient((client) =>
        client.knowledgeQuery({ subjectId: subject, queryInput: text }),
      ),
    ),
  );
cli
  .command('capture <subject> <file>')
  .description('接纳UTF-8资料')
  .action(async (subject, file) => {
    const text = await readFile(file, 'utf8');
    show(
      await withClient((client) =>
        client.materialsCapture({
          subjectId: subject,
          materialCaptureInput: {
            kind: 'document',
            mediaType: 'text/plain',
            title: file,
            text,
            source: { roots: [] },
          },
        }),
      ),
    );
  });
cli
  .command('read <subject> <id>')
  .action(async (subject, id) =>
    show(
      await withClient((client) =>
        client.knowledgeRead({ subjectId: subject, id }),
      ),
    ),
  );
cli
  .command('remember <subject> <text>')
  .description('管理员直接创建认识')
  .action(async (subject, text) =>
    show(
      await withClient((client) =>
        client.knowledgeCreate({
          subjectId: subject,
          knowledgeCreateInput: {
            kind: 'Memory',
            body: { kind: 'Memory', content: text },
            sourceRefs: [],
            formed: readReal(),
            formation: 'operator_edit',
            stance: 'accepted',
          },
        }),
      ),
    ),
  );
cli
  .command('growth <subject>')
  .action(async (subject) =>
    show(
      await withClient((client) =>
        client.growthRun({ subjectId: subject, growthRunRequest: {} }),
      ),
    ),
  );
const operation = cli
  .command('operation')
  .description('持久操作、错误和明确恢复');
operation
  .command('read <id>')
  .option('--subject <id>')
  .action(async (id, o) =>
    show(
      await withClient((client) =>
        client.operationsInspect({ id, subjectId: o.subject }),
      ),
    ),
  );
operation
  .command('trace <id>')
  .option('--subject <id>')
  .action(async (id, o) =>
    show(
      await withClient((client) =>
        client.operationsTrace({ id, subjectId: o.subject }),
      ),
    ),
  );
operation
  .command('cancel <id>')
  .option('--subject <id>')
  .action(async (id, o) =>
    show(
      await withClient((client) =>
        client.operationsCancel({
          id,
          operationControlInput: { subjectId: o.subject },
        }),
      ),
    ),
  );
operation
  .command('resume <id>')
  .option('--subject <id>')
  .action(async (id, o) =>
    show(
      await withClient((client) =>
        client.operationsResume({
          id,
          operationControlInput: { subjectId: o.subject },
        }),
      ),
    ),
  );

addManagementCommands({ cli, withClient, show });
const host = cli.command('host').description('Host生命周期');
host.command('start').action(async () => {
  const { runHost } = await import('@nous-wave/server');
  await runHost({
    programRoot,
    configPath: cli.opts().config,
    instanceHome: cli.opts().instance,
  });
});
host
  .command('status')
  .action(async () => show(await withClient((client) => client.hostStatus())));
host
  .command('drain')
  .action(async () =>
    show(await withClient((client) => client.hostDrain({ body: {} }))),
  );
host
  .command('stop')
  .option('--force')
  .action(async (options) =>
    show(
      await withClient((client) =>
        client.hostStop({
          hostStopRequest: { mode: options.force ? 'force' : 'graceful' },
        }),
      ),
    ),
  );

try {
  await cli.parseAsync();
} catch (error) {
  const failure = safeProblem(error);
  process.stderr.write(JSON.stringify(failure) + '\n');
  process.exitCode =
    failure.category === 'validation'
      ? 2
      : failure.category === 'forbidden'
        ? 3
        : failure.category === 'unavailable' ||
            failure.category === 'resource_pressure'
          ? 4
          : failure.category === 'effect_unknown'
            ? 5
            : 1;
}
