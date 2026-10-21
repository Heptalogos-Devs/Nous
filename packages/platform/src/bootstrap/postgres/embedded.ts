// Adapted from leinelissen/embedded-postgres 8c8c16d1, MIT, Copyright (c) 2022 Lei Nelissen.
// Changes: explicit acquired binaries, protected temporary password, pg_ctl lifecycle,
// no default credentials, root-account mutation, global exit hooks or compatibility fields.

import { execSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import fs from 'node:fs/promises';
import { platform, userInfo } from 'node:os';
import path from 'node:path';
import { execa } from 'execa';
import { problem } from '../../errors.js';
import { StorageClient } from '../../storage/postgres.js';
export interface PostgresBinaries {
  initdb: string;
  postgres: string;
  pg_ctl: string;
}
export interface PostgresOptions {
  binaries: PostgresBinaries;
  databaseDir: string;
  temporaryRoot: string;
  logPath: string;
  port: number;
  user: string;
  password: string;
}
function getBestLocale(): string {
  // `locale -a` is not available on Windows.
  if (platform() === 'win32') {
    return 'C';
  }
  try {
    const availableLocales = new Set(
      execSync('locale -a', { encoding: 'utf-8' })
        .split(/\r?\n/)
        .map((locale) => locale.trim())
        .filter(Boolean),
    );
    if (availableLocales.has('en_US.UTF-8')) return 'en_US.UTF-8';
    if (availableLocales.has('C.UTF-8')) return 'C.UTF-8';
    if (availableLocales.has('en_US.utf8')) return 'en_US.utf8';
  } catch {
    // Fallback to POSIX C locale
  }
  return 'C';
}

const BIN_PERMISSIONS = 0b101101101;
const ensureBinIsExecutable = async (filePath: string) => {
  // Only fix the file's mode if it's missing a permission. This is useful
  // when the binaries are in a read-only file system, as a call to chmod
  // (even unnecessary) would cause a crash.
  const stat = await fs.stat(filePath);

  if ((stat.mode & BIN_PERMISSIONS) !== BIN_PERMISSIONS) {
    await fs.chmod(filePath, stat.mode | BIN_PERMISSIONS);
  }
};

export class EmbeddedPostgres {
  constructor(private readonly options: PostgresOptions) {
    if (platform() !== 'win32' && userInfo().uid === 0)
      throw problem(
        'bootstrap.postgres_root',
        'forbidden',
        '本地PostgreSQL必须使用非root服务账号',
      );
  }
  async initialise() {
    const { binaries, databaseDir, temporaryRoot, user, password } =
      this.options;
    const locale = getBestLocale();
    await fs.mkdir(temporaryRoot, { recursive: true, mode: 0o700 });
    const passwordFile = path.join(
      temporaryRoot,
      `pg-password-${randomUUID()}`,
    );
    await fs.writeFile(passwordFile, password + '\n', {
      flag: 'wx',
      mode: 0o600,
    });
    try {
      await Promise.all([
        ensureBinIsExecutable(binaries.postgres),
        ensureBinIsExecutable(binaries.initdb),
      ]);
      await execa(
        binaries.initdb,
        [
          `--pgdata=${databaseDir}`,
          '--auth=scram-sha-256',
          `--username=${user}`,
          `--pwfile=${passwordFile}`,
          `--lc-messages=${locale}`,
        ],
        {
          env: {
            LC_MESSAGES: locale,
            TMPDIR: temporaryRoot,
            TEMP: temporaryRoot,
            TMP: temporaryRoot,
          },
          timeout: 60_000,
        },
      );
    } catch {
      throw problem(
        'bootstrap.postgres_init_failed',
        'unavailable',
        '本地PostgreSQL初始化失败',
      );
    } finally {
      await fs.rm(passwordFile, { force: true });
    }
  }
  async start() {
    const { binaries, databaseDir, port, logPath } = this.options;
    await ensureBinIsExecutable(binaries.pg_ctl);
    // All -o values are fixed literals or a validated integer; no shell path interpolation.
    const flags = `-h 127.0.0.1 -p ${port} -c log_min_error_statement=panic${platform() === 'win32' ? '' : ' -k ""'}`;
    try {
      await execa(
        binaries.pg_ctl,
        [
          'start',
          '-D',
          databaseDir,
          '-l',
          logPath,
          '-w',
          '-t',
          '30',
          '-o',
          flags,
        ],
        { timeout: 35_000 },
      );
    } catch {
      throw problem(
        'bootstrap.postgres_start_failed',
        'unavailable',
        '本地PostgreSQL启动失败，请检查实例PostgreSQL日志',
      );
    }
  }
  async stop() {
    try {
      await execa(
        this.options.binaries.pg_ctl,
        [
          'stop',
          '-D',
          this.options.databaseDir,
          '-m',
          'fast',
          '-w',
          '-t',
          '30',
        ],
        { timeout: 35_000 },
      );
    } catch {
      throw problem(
        'bootstrap.postgres_stop_failed',
        'unavailable',
        '本地PostgreSQL未在期限内完成停止',
      );
    }
  }
  client(database = 'postgres') {
    return new StorageClient({
      host: '127.0.0.1',
      port: this.options.port,
      user: this.options.user,
      password: this.options.password,
      database,
      connectionTimeoutMillis: 3000,
    });
  }
  async createDatabase(name: string) {
    const client = this.client();
    try {
      await client.connect();
      await client.query(`CREATE DATABASE ${client.escapeIdentifier(name)}`);
    } catch (error) {
      if (
        !error ||
        typeof error !== 'object' ||
        !('code' in error) ||
        error.code !== '42P04'
      )
        throw error;
    } finally {
      await client.end();
    }
  }
}
