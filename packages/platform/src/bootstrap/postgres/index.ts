import { constants } from 'node:fs';
import { access, lstat, mkdir, readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { execa } from 'execa';
import { lock } from 'proper-lockfile';
import { createBootstrapSecrets } from '../../credentials/bootstrap.js';
import { problem } from '../../errors.js';
import type { BootstrapView } from '../config.js';
import { EmbeddedPostgres, type PostgresBinaries } from './embedded.js';

const version = '18.4.0-beta.17';
const platforms = new Set([
  'linux-x64',
  'linux-arm64',
  'linux-arm',
  'linux-ia32',
  'linux-ppc64',
  'darwin-x64',
  'darwin-arm64',
  'win32-x64',
]);
export function postgresRuntimeRef(): string {
  const target = `${process.platform}-${process.arch}`;
  if (!platforms.has(target))
    throw problem(
      'bootstrap.postgres_platform_unavailable',
      'unavailable',
      '该平台尚无选定PostgreSQL发行件',
    );
  return `postgres-${version}-${target}`;
}
const platformPackage = () =>
  `@embedded-postgres/${process.platform === 'win32' ? 'windows' : process.platform}-${process.arch}`;

/** Explicit optional acquisition through the normal package manager and its lock/integrity. */
export function createPostgresRuntime(view: BootstrapView) {
  const runtimeRef = postgresRuntimeRef();
  const root = join(view.paths.runtime, runtimeRef);
  async function binaries(): Promise<PostgresBinaries> {
    try {
      const require = createRequire(join(root, 'package.json'));
      const entry = require.resolve(platformPackage());
      const inside = relative(root, entry);
      if (isAbsolute(inside) || inside.startsWith('..')) throw new Error();
      const descriptor = JSON.parse(
        await readFile(resolve(entry, '../../package.json'), 'utf8'),
      ) as { version: string; name: string };
      if (
        descriptor.version !== version ||
        descriptor.name !== platformPackage()
      )
        throw new Error();
      const loaded = (await import(
        pathToFileURL(entry).href
      )) as PostgresBinaries;
      for (const path of [loaded.initdb, loaded.postgres, loaded.pg_ctl]) {
        const local = relative(root, path);
        if (
          isAbsolute(local) ||
          local.startsWith('..') ||
          !(await lstat(path)).isFile()
        )
          throw new Error();
        await access(path, constants.R_OK);
      }
      return {
        initdb: loaded.initdb,
        postgres: loaded.postgres,
        pg_ctl: loaded.pg_ctl,
      };
    } catch {
      throw problem(
        'bootstrap.runtime_required',
        'unavailable',
        '请先显式执行runtime acquire postgres',
      );
    }
  }
  async function inspect() {
    try {
      await binaries();
      return {
        runtimeRef,
        version,
        platform: process.platform,
        arch: process.arch,
        state: 'installed' as const,
      };
    } catch {
      return {
        runtimeRef,
        version,
        platform: process.platform,
        arch: process.arch,
        state: 'missing' as const,
      };
    }
  }
  async function acquire() {
    await mkdir(root, { recursive: true, mode: 0o700 });
    const release = await lock(root, {
      retries: { retries: 5, minTimeout: 100, maxTimeout: 500 },
    });
    try {
      if ((await inspect()).state === 'installed') return inspect();
      const cache = join(view.paths.cache, 'npm');
      // Only the selected native package has an install hook; upstream hydrates its shipped symlinks.
      // npm owns dependency resolution, package-lock and registry integrity verification.
      try {
        await execa(
          'npm',
          [
            'install',
            '--prefix',
            root,
            '--no-audit',
            '--no-fund',
            '--save-exact',
            `${platformPackage()}@${version}`,
          ],
          {
            env: {
              npm_config_cache: cache,
              TMPDIR: view.paths.temp,
              TEMP: view.paths.temp,
              TMP: view.paths.temp,
            },
            timeout: 300_000,
            maxBuffer: 1024 * 1024,
          },
        );
      } catch {
        throw problem(
          'bootstrap.runtime_acquire_failed',
          'unavailable',
          'PostgreSQL发行件取得失败，请检查包管理器与网络配置',
        );
      }
      await binaries();
      return inspect();
    } finally {
      await release();
    }
  }
  return { runtimeRef, inspect, acquire, binaries };
}

/** Starts only this installation's managed cluster. External PostgreSQL is never stopped. */
export async function withBootstrapDatabase<T>(
  view: BootstrapView,
  use: (databaseURL: string) => Promise<T>,
): Promise<T> {
  const database = view.bootstrap.database,
    secrets = createBootstrapSecrets(view.paths.secret);
  if (database.mode === 'external')
    return secrets.withSecret(
      database.urlRef,
      { purpose: 'bootstrap.database' },
      (value) => use(new TextDecoder().decode(value)),
    );
  const runtime = createPostgresRuntime(view);
  if (database.runtimeRef !== runtime.runtimeRef)
    throw problem(
      'bootstrap.runtime_mismatch',
      'conflict',
      '引导配置指向不同平台或版本的PostgreSQL运行时',
    );
  const binaries = await runtime.binaries();
  return secrets.withSecret(
    database.passwordRef,
    { purpose: 'bootstrap.database' },
    async (bytes) => {
      const password = new TextDecoder().decode(bytes);
      const cluster = new EmbeddedPostgres({
        binaries,
        databaseDir: join(view.paths.data, 'postgres'),
        temporaryRoot: view.paths.secret,
        logPath: join(view.paths.log, 'postgres.log'),
        port: database.port,
        user: database.user,
        password,
      });
      const url = new URL('postgresql://127.0.0.1');
      url.port = String(database.port);
      url.username = database.user;
      url.password = password;
      url.pathname = `/${database.database}`;
      let owned = false;
      const release = await lock(view.paths.data, {
        lockfilePath: join(view.paths.run, 'postgres-start.lock'),
        retries: { retries: 20, minTimeout: 100, maxTimeout: 500 },
      });
      try {
        const probe = cluster.client();
        let running = false;
        try {
          await probe.connect();
          running = true;
        } catch {
        } finally {
          await probe.end();
        }
        if (!running) {
          let initialized = false;
          try {
            await access(join(view.paths.data, 'postgres', 'PG_VERSION'));
            initialized = true;
          } catch {}
          if (!initialized) await cluster.initialise();
          await cluster.start();
          owned = true;
        }
      } finally {
        await release();
      }
      try {
        await cluster.createDatabase(database.database);
        return await use(url.href);
      } finally {
        if (owned) await cluster.stop();
      }
    },
  );
}
