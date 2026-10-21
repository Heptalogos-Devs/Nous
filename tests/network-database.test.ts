import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { randomUUID } from 'node:crypto';
import { createStorage, type Kysely } from '@nous-wave/platform/storage';
import {
  createSettings,
  settingsMigration,
  type SettingsDatabase,
} from '@nous-wave/platform/settings';
import {
  createNetwork,
  networkSettingsModule,
} from '@nous-wave/platform/network';
import type { CallContext } from '@nous-wave/contracts/common';

// Dedicated PostgreSQL is selected explicitly; each run owns one temporary schema.
test('network: saved profile drives native multipart, Request streams, redirects and cancellation', {
  skip: !process.env.NOUS_NETWORK_TEST_DATABASE_URL,
}, async () => {
  const connectionString = process.env.NOUS_NETWORK_TEST_DATABASE_URL!;
  const schema = `network_${randomUUID().replaceAll('-', '')}`,
    installationId = schema;
  const admin = createStorage<SettingsDatabase>({
    pool: { connectionString },
    datasourceName: `${schema}-admin`,
    runDatasourceMigrations: false,
  });
  await admin.db.schema.createSchema(schema).execute();
  const storage = createStorage<SettingsDatabase>({
    pool: { connectionString, options: `-c search_path=${schema}` },
    datasourceName: schema,
    runDatasourceMigrations: false,
  });
  let network: ReturnType<typeof createNetwork> | undefined;
  let requests = 0,
    cancelled = false;
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const server = createServer((request, response) => {
    requests++;
    if (request.url === '/api/redirect') {
      response.writeHead(302, { location: '/outside' });
      response.end();
      return;
    }
    if (request.url === '/api/stream') {
      response.write('first');
      const timer = setTimeout(() => {
        timers.delete(timer);
        response.end('last');
      }, 1000);
      timers.add(timer);
      response.once('close', () => {
        cancelled = !response.writableEnded;
        clearTimeout(timer);
        timers.delete(timer);
      });
      return;
    }
    const chunks: Buffer[] = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.once('end', () =>
      response.end(
        JSON.stringify({
          type: request.headers['content-type'],
          body: Buffer.concat(chunks).toString(),
        }),
      ),
    );
  });
  try {
    await settingsMigration.up(storage.db as Kysely<unknown>);
    const ctx: CallContext = {
      actor: {
        kind: 'service',
        id: 'network-test-host',
        authorityRef: 'protected-process:network-test',
      },
      consumer: { kind: 'maintenance', host: 'nous', id: installationId },
      purpose: 'network-test',
      audience: [],
      trace: { traceId: 'f'.repeat(32), operationId: 'network-profile' },
    };
    const settings = createSettings({
      installationId,
      db: storage.db,
      modules: [networkSettingsModule],
      authorize(context) {
        assert.equal(context.actor.authorityRef, ctx.actor.authorityRef);
      },
    });
    await settings.registerDefinitions();
    const profile = structuredClone(
      networkSettingsModule.defaults.profiles.default,
    );
    profile.proxy = { mode: 'direct' };
    await settings.update(ctx, {
      namespace: 'platform.network',
      scope: { kind: 'installation', installationId },
      expectedRevision: 0,
      patch: [{ op: 'add', path: '/profiles', value: { default: profile } }],
    });
    network = createNetwork({
      installationId,
      product: 'nous',
      processBootId: randomUUID(),
      settings,
      authorize: async (context, operation, target) => {
        assert.equal(context.actor.authorityRef, ctx.actor.authorityRef);
        assert.equal(operation, 'resources.read');
        assert.equal(target.installationId, installationId);
      },
    });
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const address = server.address();
    assert.ok(address && typeof address === 'object');
    const base = `http://127.0.0.1:${address.port}`;
    const resource = {
      resourceRef: {
        product: 'nous' as const,
        installationId,
        kind: 'Resource',
        id: 'test-http-resource',
      },
      operation: 'resources.read',
      allowedOrigins: [base],
      allowedPathPrefixes: ['/api'],
      methods: ['GET', 'POST'],
      addressScope: 'configured_private' as const,
      allowedPrivateRanges: ['127.0.0.0/8'],
      credentialOrigins: [],
    };
    await network.withClient(ctx, { resource }, async (client) => {
      const form = new FormData();
      form.set('text', '保存配置后的真实multipart');
      form.set('file', new Blob(['file-content']), 'scene.txt');
      const response = await client.fetch(`${base}/api/echo`, {
        method: 'POST',
        body: form,
      });
      const clone = response.clone();
      assert.equal(clone.url, response.url);
      await clone.text();
      const result = await response.json();
      assert.match(result.type, /multipart\/form-data; boundary=/);
      assert.match(result.body, /file-content/);
      const request = new Request(`${base}/api/echo`, {
        method: 'POST',
        body: new ReadableStream({
          start(controller) {
            controller.enqueue(new TextEncoder().encode('stream-upload'));
            controller.close();
          },
        }),
        duplex: 'half',
      } as RequestInit & { duplex: 'half' });
      assert.match(
        (await (await client.fetch(request)).json()).body,
        /stream-upload/,
      );
      const before = requests;
      await assert.rejects(client.fetch(`${base}/outside`), {
        code: 'network.unauthorized_target',
      });
      await assert.rejects(client.fetch(`${base}/api/redirect`), {
        code: 'network.redirect_denied',
      });
      assert.equal(requests, before + 1);
      const streamed = await client.fetch(`${base}/api/stream`),
        reader = streamed.body!.getReader();
      assert.equal(
        new TextDecoder().decode((await reader.read()).value),
        'first',
      );
      await reader.cancel();
    });
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.equal(cancelled, true);
    const state = await settings.applied(ctx, {
      namespaces: ['platform.network'],
      scope: { kind: 'installation', installationId },
    });
    assert.equal(state[0].fields['/profiles'].state, 'current');
  } finally {
    await network?.close();
    for (const timer of timers) clearTimeout(timer);
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await storage.close();
    await admin.db.schema.dropSchema(schema).cascade().execute();
    await admin.close();
  }
});
