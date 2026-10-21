// Actual adoption is reported by consumers; saved revisions do not assert running state.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { z } from 'zod';
import type { Problem } from '@nous-wave/contracts/common';
import type { Client } from 'pg';
import type { Kysely } from 'kysely';
import { problem, safeProblem } from '../errors.js';
import { at, digest, immutable, scopeKey } from './catalog.js';
import type { createCatalog } from './catalog.js';
import { ConfigScopeSchema } from './types.js';
import type {
  AppliedField,
  ConfigScope,
  ConfigSnapshot,
  ConsumerAppliedState,
  ConsumerAppliedView,
  EffectiveSettingsView,
  SettingsEvent,
  SettingsNotification,
} from './types.js';
const notificationSchema = z.strictObject({
  namespace: z.string(),
  scope: ConfigScopeSchema,
  revision: z.number().int().nonnegative().safe(),
});

export function createRuntimeState(
  catalog: ReturnType<typeof createCatalog>,
  onError?: (problem: Problem) => void,
) {
  const listeners = new Set<(event: SettingsEvent) => void | Promise<void>>();
  const reports = new Map<string, ConsumerAppliedState>();
  function emit(event: SettingsEvent) {
    for (const listener of listeners)
      void Promise.resolve()
        .then(() => listener(event))
        .catch((error) =>
          onError?.(safeProblem(error, 'settings.consumer_failed')),
        );
  }
  function changed(change: SettingsNotification) {
    if (!catalog.ids.includes(change.namespace)) return;
    emit(immutable({ kind: 'changed', change: structuredClone(change) }));
  }
  function subscribe(listener: (event: SettingsEvent) => void | Promise<void>) {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }
  function registerConsumer(
    input: {
      processBootId: string;
      consumerId: string;
      namespace: string;
      scope: ConfigScope;
    },
    readSnapshot: (database?: Kysely<any>) => Promise<ConfigSnapshot>,
  ) {
    const module = catalog.module(input.namespace);
    if (!input.processBootId || !input.consumerId)
      throw problem(
        'settings.invalid_consumer',
        'validation',
        '配置消费者需要真实boot和consumer标识',
      );
    const key = digest([
      input.processBootId,
      input.consumerId,
      input.namespace,
      input.scope,
    ]);
    if (reports.has(key))
      throw problem(
        'settings.consumer_conflict',
        'conflict',
        '配置消费者已注册',
      );
    const paths = Object.entries(module.fields)
      .filter(([, field]) => field.consumerIds.includes(input.consumerId))
      .map(([path]) => path);
    if (!paths.length)
      throw problem(
        'settings.consumer_not_declared',
        'validation',
        '配置定义未声明该消费者',
      );
    const state: ConsumerAppliedState = {
      ...structuredClone(input),
      fields: {},
    };
    reports.set(key, state);
    let closed = false;
    function active() {
      if (closed)
        throw problem(
          'settings.consumer_closed',
          'conflict',
          '配置消费者已关闭',
        );
    }
    return {
      async snapshot(database?: Kysely<any>): Promise<ConfigSnapshot> {
        active();
        const snapshot = await readSnapshot(database);
        active();
        return snapshot;
      },
      reportApplied(
        snapshot: ConfigSnapshot,
        selected: readonly string[] = paths,
      ) {
        active();
        if (
          snapshot.revisions.some((ref) => ref.namespace !== input.namespace) ||
          !snapshot.revisions.some(
            (ref) => scopeKey(ref.scope) === scopeKey(input.scope),
          )
        )
          throw problem(
            'settings.snapshot_mismatch',
            'validation',
            '采用快照不属于当前消费者范围',
          );
        for (const path of selected) {
          if (!paths.includes(path))
            throw problem(
              'settings.consumer_field_forbidden',
              'forbidden',
              '消费者未声明该配置字段',
              { details: { path } },
            );
          const field = at(snapshot.values, path);
          state.fields[path] = immutable({
            definitionRevision: snapshot.definitionRevision,
            revisions: structuredClone(snapshot.revisions),
            ...(field.found ? { value: structuredClone(field.value!) } : {}),
          });
        }
        delete state.error;
      },
      reportFailure(error: Problem, unavailablePaths: readonly string[] = []) {
        active();
        for (const path of unavailablePaths) {
          if (!paths.includes(path))
            throw problem(
              'settings.consumer_field_forbidden',
              'forbidden',
              '消费者未声明该配置字段',
              { details: { path } },
            );
          delete state.fields[path];
        }
        state.error = immutable(structuredClone(error));
      },
      close() {
        closed = true;
        reports.delete(key);
      },
    };
  }
  function applied(desired: EffectiveSettingsView): ConsumerAppliedView[] {
    return immutable(
      [...reports.values()]
        .filter(
          (report) =>
            scopeKey(report.scope) === scopeKey(desired.scope) &&
            desired.modules[report.namespace],
        )
        .map((report) => {
          const snapshot = desired.modules[report.namespace];
          const fields = Object.fromEntries(
            Object.entries(catalog.module(report.namespace).fields)
              .filter(([, field]) =>
                field.consumerIds.includes(report.consumerId),
              )
              .map(([path, metadata]) => {
                const current = report.fields[path];
                const value = at(snapshot.values, path);
                const expected: AppliedField = {
                  definitionRevision: snapshot.definitionRevision,
                  revisions: snapshot.revisions,
                  ...(value.found ? { value: value.value! } : {}),
                };
                const matches =
                  current &&
                  current.definitionRevision === expected.definitionRevision &&
                  digest({
                    value: current.value ?? null,
                    found: Object.hasOwn(current, 'value'),
                  }) ===
                    digest({
                      value: expected.value ?? null,
                      found: Object.hasOwn(expected, 'value'),
                    });
                const field: ConsumerAppliedView['fields'][string] = {
                  effect: metadata.effect,
                  state: matches
                    ? 'current'
                    : report.error
                      ? 'failed'
                      : current
                        ? 'pending'
                        : 'unknown',
                  desired: expected,
                  ...(current ? { applied: current } : {}),
                };
                return [path, field];
              }),
          );
          return { ...structuredClone(report), fields };
        }),
    );
  }
  async function listen(createSession: () => Client) {
    let client: Client | undefined;
    let closed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let connecting: Promise<void> | undefined;
    let delay = 250;
    let initial = true;
    function schedule() {
      if (closed || timer) return;
      timer = setTimeout(() => {
        timer = undefined;
        void connect().catch((error) =>
          onError?.(safeProblem(error, 'settings.notifications_unavailable')),
        );
      }, delay);
      timer.unref();
      delay = Math.min(delay * 2, 30_000);
    }
    function disconnect(current: Client) {
      if (client !== current) return;
      client = undefined;
      current.removeAllListeners('notification');
      current.removeListener('error', lost);
      current.removeListener('end', ended);
      void current.end().catch(() => {});
      schedule();
    }
    function lost() {
      if (client) disconnect(client);
    }
    function ended() {
      if (client) disconnect(client);
    }
    function connect(): Promise<void> {
      if (connecting) return connecting;
      connecting = (async () => {
        let current: Client | undefined;
        try {
          current = createSession();
          if (closed) {
            await current.end();
            return;
          }
          client = current;
          current.on('error', lost);
          current.on('end', ended);
          current.on('notification', (notification) => {
            if (notification.channel !== 'settings_changed') return;
            try {
              changed(
                notificationSchema.parse(
                  JSON.parse(notification.payload ?? ''),
                ),
              );
            } catch {
              onError?.(
                problem(
                  'settings.invalid_notification',
                  'failed',
                  '配置通知格式无效',
                ).problem,
              );
            }
          });
          await current.connect();
          await current.query('LISTEN settings_changed');
          if (closed || client !== current) return;
          delay = 250;
          emit({ kind: 'resync' });
        } catch {
          if (current && client === current) disconnect(current);
          else schedule();
          if (initial)
            throw problem(
              'settings.notifications_unavailable',
              'unavailable',
              '配置通知连接不可用',
            );
          onError?.(
            problem(
              'settings.notifications_unavailable',
              'unavailable',
              '配置通知连接不可用',
            ).problem,
          );
        }
      })().finally(() => {
        connecting = undefined;
      });
      return connecting;
    }
    try {
      await connect();
    } catch (error) {
      closed = true;
      if (timer) clearTimeout(timer);
      throw error;
    }
    initial = false;
    return {
      async close() {
        closed = true;
        if (timer) clearTimeout(timer);
        await connecting;
        const current = client;
        client = undefined;
        if (!current) return;
        current.removeAllListeners('notification');
        current.removeListener('error', lost);
        current.removeListener('end', ended);
        try {
          await current.query('UNLISTEN settings_changed');
          await current.end();
        } catch {
          void current.end().catch(() => {});
        }
      },
    };
  }
  return { changed, subscribe, registerConsumer, applied, listen };
}
