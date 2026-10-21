import type { SettingsModule } from '../settings/index.js';
import { NetworkSettingsSchema, type NetworkSettings } from './contracts.js';
export const networkSettingsModule: SettingsModule<NetworkSettings> = {
  id: 'platform.network',
  definitionRevision: '1',
  owner: 'platform.network',
  schema: NetworkSettingsSchema,
  defaults: {
    defaultProfileId: 'default',
    profiles: {
      default: {
        proxy: { mode: 'environment' },
        tls: { useSystemCA: true, caFileRefs: [], rejectUnauthorized: true },
        timeout: {
          connectMs: 10000,
          headersMs: 300000,
          bodyIdleMs: 30000,
          totalMs: 120000,
        },
        connections: 8,
        pipelining: 1,
        maxHeaderBytes: 16384,
        redirects: { mode: 'deny', maxHops: 5 },
        maxResponseBytes: 64 * 1024 * 1024,
      },
    },
  },
  fields: {
    '/defaultProfileId': {
      labelKey: 'network.defaultProfile',
      descriptionKey: 'network.defaultProfile.description',
      scopes: ['installation', 'subject', 'role'],
      visibility: 'standard',
      effect: 'next_call',
      consumerIds: ['network.factory'],
    },
    '/profiles': {
      labelKey: 'network.profiles',
      descriptionKey: 'network.profiles.description',
      scopes: ['installation'],
      visibility: 'advanced',
      effect: 'next_call',
      consumerIds: ['network.factory'],
    },
  },
};
