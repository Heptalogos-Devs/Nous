import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import {
  Configuration,
  HostApi,
  problemMiddleware,
} from '@nous-wave/client';
import { createBootstrapSecrets } from '@nous-wave/platform/credentials';
import { execa } from 'execa';
import { Pool } from 'pg';

// One actual installed-product scenario. PostgreSQL is explicit; each run owns a fresh database.
test('独立CLI安装、配置修订、密钥撤销、重新登录和Host重启贯通', {
  skip: !process.env.NOUS_TEST_DATABASE_URL,
  timeout: 120_000,
}, async () => {
  const name = `nous_install_${randomBytes(6).toString('hex')}`;
  const admin = new Pool({
    connectionString: process.env.NOUS_TEST_DATABASE_URL,
    max: 1,
  });
  const url = new URL(process.env.NOUS_TEST_DATABASE_URL!);
  url.pathname = `/${name}`;
  const root = resolve('../temp/tests');
  await mkdir(root, { recursive: true });
  const directory = await mkdtemp(join(root, 'installation-'));
  let password = randomBytes(24).toString('base64url');
  const env = {
    ...process.env,
    NOUS_DATABASE_URL: url.toString(),
    NOUS_ADMIN_PASSWORD: password,
  };
  const cli = async (args: string[]) =>
    JSON.parse(
      (
        await execa(
          process.execPath,
          ['apps/cli/dist/main.js', '--instance', directory, '--json', ...args],
          { env },
        )
      ).stdout,
    );
  let host: ReturnType<typeof execa> | undefined;
  try {
    await admin.query(`CREATE DATABASE "${name}"`);
    const listener = createServer();
    await new Promise<void>((done) => listener.listen(0, '127.0.0.1', done));
    const address = listener.address();
    assert.ok(address && typeof address !== 'string');
    await new Promise<void>((done, reject) =>
      listener.close((error) => (error ? reject(error) : done())),
    );
    const initialized = await cli(['init', '--port', String(address.port)]);
    assert.equal(initialized.subjectStarted, false);
    assert.equal(initialized.modelCalled, false);
    await cli(['auth', 'init', '--email', 'platform@example.test']);
    let apiKey = await createBootstrapSecrets(
      initialized.paths.secret,
    ).withSecret('cli-auth', { purpose: 'cli.authentication' }, async (value) =>
      new TextDecoder().decode(value),
    );
    const baseURL = `http://127.0.0.1:${address.port}`;
    async function start() {
      host = execa(process.execPath, ['apps/server/dist/main.js'], {
        env: { ...env, NOUS_CONFIG: initialized.configPath },
        reject: false,
        killSignal: 'SIGTERM',
        forceKillAfterDelay: 10_000,
      });
      let output = '',
        diagnostics = '',
        exited = false;
      host.nodeChildProcess.once('exit', () => {
        exited = true;
      });
      host.stderr!.on('data', (chunk) => {
        diagnostics += chunk.toString();
      });
      host.stdout!.on('data', (chunk) => {
        output += chunk.toString();
      });
      for (let attempt = 0; attempt < 400; attempt++) {
        if (output.includes('"event":"host.started"')) return;
        if (exited)
          throw new Error(`Host在监听前退出: ${diagnostics.slice(-8000)}`);
        await delay(25);
      }
      throw new Error('Host启动超时');
    }
    async function stop() {
      if (host) {
        host.kill('SIGTERM');
        const result = await host;
        assert.equal(result.exitCode, 0);
        host = undefined;
      }
    }
    const call = async (
      method: string,
      path: string,
      body?: unknown,
      submissionId?: string,
    ) => {
      const response = await fetch(baseURL + path, {
        method,
        headers: {
          authorization: `Bearer ${apiKey}`,
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          ...(submissionId ? { 'idempotency-key': submissionId } : {}),
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      return { status: response.status, body: await response.json() };
    };
    await start();
    const client = new HostApi(
      new Configuration({
        basePath: baseURL,
        accessToken: () => apiKey,
        middleware: [problemMiddleware],
      }),
    );
    assert.equal(
      (await client.hostStatus()).installationId,
      initialized.installationId,
    );
    assert.deepEqual((await call('GET', '/v1/subjects')).body, { items: [] });
    const scope = {
      kind: 'installation',
      installationId: initialized.installationId,
    };
    const query = new URLSearchParams({
      'scope[kind]': 'installation',
      'scope[installationId]': initialized.installationId,
      namespaces: 'growth',
    });
    const change = {
      namespace: 'growth',
      scope,
      expectedRevision: 0,
      patch: [{ op: 'add', path: '/batchSize', value: 16 }],
    };
    const changed = await call('PATCH', '/v1/settings', change, 'role-output');
    assert.equal(changed.status, 200);
    assert.equal(changed.body.snapshot.values.batchSize, 16);
    assert.deepEqual(
      (await call('PATCH', '/v1/settings', change, 'role-output')).body,
      changed.body,
    );
    assert.equal(
      (
        await call(
          'PATCH',
          '/v1/settings',
          { ...change, patch: [{ op: 'add', path: '/batchSize', value: 99 }] },
          'role-output',
        )
      ).status,
      409,
    );
    const read = await call('GET', `/v1/settings?${query}`);
    assert.equal(read.status, 200);
    assert.deepEqual(read.body.targetOverrides.growth.sparse, {
      batchSize: 16,
    });
    const resourceRef = {
      product: 'nous',
      installationId: initialized.installationId,
      kind: 'Provider',
      id: 'test-provider',
    };
    const secret = await call('POST', '/v1/secrets', {
      purpose: 'model.provider',
      resourceRef,
      allowedConsumers: [
        { kind: 'maintenance', host: 'nous', id: initialized.installationId },
      ],
      backend: { kind: 'protected_file', material: 'PRIVATE-test-material' },
    });
    assert.equal(secret.status, 200);
    assert.equal(secret.body.available, true);
    assert.equal(JSON.stringify(secret.body).includes('PRIVATE'), false);
    const rotated = await call(
      'POST',
      `/v1/secrets/${encodeURIComponent(secret.body.ref)}/rotate`,
      {
        expectedRevision: 1,
        backend: { kind: 'protected_file', material: 'PRIVATE-replacement' },
      },
    );
    assert.equal(rotated.body.generation, 2);
    assert.equal(rotated.body.cleanupPending, false);
    const revoked = await call(
      'POST',
      `/v1/secrets/${encodeURIComponent(secret.body.ref)}/revoke`,
      { expectedRevision: 2 },
    );
    assert.equal(revoked.body.state, 'revoked');
    assert.equal(revoked.body.available, false);
    await stop();
    password = randomBytes(24).toString('base64url');
    env.NOUS_ADMIN_PASSWORD = password;
    const recovered = await cli([
      'recovery',
      'reset-administrator',
      '--email',
      'platform@example.test',
    ]);
    assert.equal(recovered.passwordReset, true);
    await start();
    assert.equal(
      (await client.hostStatus()).installationId,
      initialized.installationId,
    );
    assert.equal(
      (await call('GET', `/v1/settings?${query}`)).body.modules.growth.values
        .batchSize,
      16,
    );
    const login = await fetch(baseURL + '/v1/auth/sign-in/email', {
      method: 'POST',
      headers: { 'content-type': 'application/json', origin: baseURL },
      body: JSON.stringify({ email: 'platform@example.test', password }),
    });
    assert.equal(login.status, 200);
    const cookie = login.headers
      .getSetCookie()
      .map((value) => value.split(';')[0])
      .join('; ');
    const issued = await fetch(baseURL + '/v1/management-keys', {
      method: 'POST',
      headers: { 'content-type': 'application/json', cookie, origin: baseURL },
      body: JSON.stringify({ name: 'revocation probe' }),
    });
    assert.equal(issued.status, 200);
    const short = await issued.json();
    assert.equal(
      (
        await call('POST', `/v1/delegations/${short.delegationId}/revoke`, {
          expectedRevision: 1,
        })
      ).status,
      200,
    );
    assert.equal(
      (
        await fetch(baseURL + '/v1/status', {
          headers: { authorization: `Bearer ${short.key}` },
        })
      ).status,
      403,
    );
    await cli(['auth', 'login', '--email', 'platform@example.test']);
    apiKey = await createBootstrapSecrets(initialized.paths.secret).withSecret(
      'cli-auth',
      { purpose: 'cli.authentication' },
      async (value) => new TextDecoder().decode(value),
    );
    assert.equal((await call('GET', '/v1/status')).status, 200);
    await stop();
  } finally {
    if (host) {
      host.kill('SIGTERM');
      await host;
    }
    await admin.query(`DROP DATABASE IF EXISTS "${name}" WITH (FORCE)`);
    await admin.end();
    await rm(directory, { recursive: true, force: true });
  }
});
