import {
  createSettings,
  type SettingsModule,
  type FieldMetadata,
} from '@nous-wave/platform/settings';
import {
  GrowthSettingsSchema,
  RetrievalSettingsSchema,
  type GrowthSettings,
  type RetrievalSettings,
} from '@nous-wave/contracts/management';
import type { Storage } from '../platform/context.js';
import { requireSubject } from '../platform/security.js';
import { fail } from '../platform/errors.js';

export const growthDefaults: Readonly<GrowthSettings> = {
  enabled: true,
  batchSize: 24,
  intervalMinutes: 30,
  journalEnabled: true,
  journalTimeZone: 'UTC',
};
export const retrievalDefaults: Readonly<RetrievalSettings> = {
  limit: 12,
  exploration: 2,
};
const field = (
  namespace: string,
  key: string,
  scopes: FieldMetadata['scopes'],
  consumerIds: string[],
  restriction?: FieldMetadata['restriction'],
): FieldMetadata => ({
  labelKey: `settings.${namespace}.${key}.label`,
  descriptionKey: `settings.${namespace}.${key}.description`,
  scopes,
  visibility: 'standard',
  effect: 'next_call',
  consumerIds,
  ...(restriction ? { restriction } : {}),
});
const subject: FieldMetadata['scopes'] = ['installation', 'subject'];
const fields = (
  namespace: string,
  keys: string[],
  scopes: FieldMetadata['scopes'],
  consumer: string,
  restriction?: FieldMetadata['restriction'],
) =>
  Object.fromEntries(
    keys.map((key) => [
      `/${key}`,
      field(namespace, key, scopes, [consumer], restriction),
    ]),
  );
export const productSettingsModules: readonly SettingsModule[] = [
  {
    id: 'growth',
    definitionRevision: 'nous.growth.1',
    owner: 'cognition',
    schema: GrowthSettingsSchema,
    defaults: growthDefaults,
    fields: fields(
      'growth',
      Object.keys(GrowthSettingsSchema.shape),
      subject,
      'growth',
    ),
  },
  {
    id: 'retrieval',
    definitionRevision: 'nous.retrieval.1',
    owner: 'retrieval',
    schema: RetrievalSettingsSchema,
    defaults: retrievalDefaults,
    fields: fields(
      'retrieval',
      Object.keys(RetrievalSettingsSchema.shape),
      subject,
      'retrieval',
    ),
  },
];

/** Product definitions and authority are composed once; consumers use the shared API. */
export function createProductSettings(
  store: Storage,
  options: { extraModules?: readonly SettingsModule[] } = {},
) {
  return createSettings({
    installationId: store.installationId,
    db: store.db,
    modules: [...productSettingsModules, ...(options.extraModules ?? [])],
    authorize: async (ctx, request, client) => {
      if (request.scope.kind === 'installation') {
        if (request.scope.installationId !== store.installationId)
          fail('forbidden', '设置不属于当前安装');
      } else {
        requireSubject(ctx);
        if (request.scope.subjectId !== ctx.subjectId)
          fail('forbidden', '设置不属于当前主体');
      }
      await store.authorize(
        ctx,
        `settings.${request.action}`,
        undefined,
        client,
      );
    },
  });
}
export type ProductSettings = ReturnType<typeof createProductSettings>;
