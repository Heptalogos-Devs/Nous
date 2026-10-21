import { z } from 'zod';
import {
  RefSchema,
  type CallContext,
  type Ref,
  type TimePoint,
  type Problem,
} from '@nous-wave/contracts/common';
import type {
  createSettings,
  ConfigScope,
  RevisionRef,
} from '../settings/index.js';
import type {
  SecretUse,
  SecretUseMetadata,
} from '@nous-wave/contracts/credentials';
const cleanUrl = z.url().refine((value) => {
  const url = new URL(value);
  return !url.username && !url.password && !url.hash && !url.search;
}, '网络配置URL不能带凭据或query');
export const NetworkProfileSchema = z.strictObject({
  proxy: z.discriminatedUnion('mode', [
    z.strictObject({ mode: z.literal('direct') }),
    z.strictObject({
      mode: z.literal('environment'),
      noProxy: z.string().optional(),
    }),
    z.strictObject({
      mode: z.literal('http'),
      url: cleanUrl.refine((value) =>
        ['http:', 'https:'].includes(new URL(value).protocol),
      ),
      credentialRef: z.string().optional(),
    }),
    z.strictObject({
      mode: z.literal('socks5'),
      url: cleanUrl.refine((value) =>
        ['socks:', 'socks5:'].includes(new URL(value).protocol),
      ),
      credentialRef: z.string().optional(),
    }),
  ]),
  tls: z.strictObject({
    useSystemCA: z.boolean(),
    caFileRefs: z.array(z.string().min(1)),
    clientCertificateRef: z.string().optional(),
    clientKeyRef: z.string().optional(),
    rejectUnauthorized: z.boolean(),
  }),
  timeout: z.strictObject({
    connectMs: z.number().int().min(1).max(3600000),
    headersMs: z.number().int().min(1).max(3600000),
    bodyIdleMs: z.number().int().min(1).max(3600000),
    totalMs: z.number().int().min(1).max(86400000),
  }),
  connections: z.number().int().min(1).max(1024),
  pipelining: z.number().int().min(1).max(32),
  maxHeaderBytes: z.number().int().min(1024).max(1048576),
  redirects: z.strictObject({
    mode: z.enum(['deny', 'follow']),
    maxHops: z.number().int().min(0).max(20),
  }),
  maxResponseBytes: z.number().int().min(1).max(2147483647),
});
export const NetworkSettingsSchema = z
  .strictObject({
    defaultProfileId: z.string().min(1),
    profiles: z.record(z.string().min(1), NetworkProfileSchema),
  })
  .refine(
    (value) => Boolean(value.profiles[value.defaultProfileId]),
    '默认网络profile必须存在',
  );
export type NetworkProfile = z.infer<typeof NetworkProfileSchema>;
export type NetworkSettings = z.infer<typeof NetworkSettingsSchema>;
export const ResourceNetworkScopeSchema = z.strictObject({
  resourceRef: RefSchema,
  operation: z.string().min(1),
  profileId: z.string().min(1).optional(),
  allowedOrigins: z
    .array(cleanUrl.transform((value) => new URL(value).origin))
    .min(1),
  allowedPathPrefixes: z.array(z.string().startsWith('/')).min(1),
  methods: z.array(z.string().regex(/^[A-Z]+$/)).min(1),
  addressScope: z.enum(['public', 'configured_private']),
  allowedPrivateRanges: z.array(z.string()).optional(),
  credentialOrigins: z.array(
    cleanUrl.transform((value) => new URL(value).origin),
  ),
});
export type ResourceNetworkScope = z.infer<typeof ResourceNetworkScopeSchema>;
export interface NetworkDiagnostic {
  resourceRef: Ref;
  profileId: string;
  phase: 'prepare' | 'dispatch' | 'headers' | 'body' | 'complete';
  bytesRead: number;
  status?: number;
  problem?: Problem;
}
export interface NetworkOptions {
  product: 'nous' | 'hept';
  installationId: string;
  processBootId: string;
  settings: Pick<
    ReturnType<typeof createSettings>,
    'registerConsumer' | 'subscribe'
  >;
  authorize(ctx: CallContext, operation: string, target: Ref): Promise<void>;
  /** Re-read the owning resource; no network directory or caller supplied operation is authoritative. */
  resolveResourceNetwork?(
    ctx: CallContext,
    resourceRef: Ref,
  ): Promise<NetworkSelection>;
  withMaterial?<T>(
    ctx: CallContext,
    input: SecretUse,
    use: (bytes: Uint8Array, metadata: SecretUseMetadata) => Promise<T>,
  ): Promise<T>;
  /** Authoritative connector confirms a remote-DNS proxy enforces this address scope. */
  authorizeProxyRoute?(
    ctx: CallContext,
    input: {
      profile: NetworkProfile;
      resource: ResourceNetworkScope;
      url: URL;
    },
  ): Promise<void>;
  onDiagnostic?: (event: NetworkDiagnostic) => void;
  maxPools?: number;
}
export interface NetworkSelection {
  resource: ResourceNetworkScope;
  scope?: ConfigScope;
  deadline?: TimePoint;
}
export interface NetworkMetadata {
  scope: ConfigScope;
  resourceRef: Ref;
  profileId: string;
  revisionRefs: RevisionRef[];
  settingsDefinitionRevision: string;
}
