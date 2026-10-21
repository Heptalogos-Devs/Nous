// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { z } from 'zod';
import type { SettingsModule } from '../settings/types.js';
export const LogLevelSchema = z.enum(['trace','debug','info','warn','error','fatal','silent']);
const endpoint = z.url().refine(value => { const url = new URL(value); return ['http:','https:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash; }, 'OTLP地址须为不含凭据、query或fragment的HTTP(S)地址');
export const TelemetrySettingsSchema = z.strictObject({
  level: LogLevelSchema,
  components: z.record(z.string().regex(/^[a-zA-Z0-9_.:-]{1,128}$/), LogLevelSchema),
  console: z.strictObject({ enabled: z.boolean() }),
  file: z.strictObject({ enabled: z.boolean(), sizeBytes: z.number().int().min(1024).max(1024 ** 4), frequency: z.union([z.enum(['daily','hourly']),z.number().int().min(1000)]), historyCount: z.number().int().positive().max(10_000) }),
  otlp: z.strictObject({ enabled: z.boolean(), protocol: z.enum(['http/protobuf','http/json','grpc']), endpoint: endpoint.nullable(), headersRef: z.string().min(1).max(512).nullable(), timeoutMs: z.number().int().positive().max(300_000), queueSize: z.number().int().positive().max(100_000), batchSize: z.number().int().positive().max(100_000), delayMs: z.number().int().positive().max(300_000), sampleRatio: z.number().min(0).max(1), metricIntervalMs: z.number().int().min(1000).max(3_600_000) }),
}).superRefine((value, ctx) => {
  if (value.otlp.enabled && !value.otlp.endpoint) ctx.addIssue({code:'custom',path:['otlp','endpoint'],message:'启用OTLP需要地址'});
  if (value.otlp.batchSize > value.otlp.queueSize) ctx.addIssue({code:'custom',path:['otlp','batchSize'],message:'batchSize不能超过queueSize'});
  if (value.otlp.protocol === 'grpc' && value.otlp.headersRef && value.otlp.endpoint?.startsWith('http:')) ctx.addIssue({code:'custom',path:['otlp','endpoint'],message:'带身份的gRPC导出需要TLS'});
});
export type TelemetrySettings = z.infer<typeof TelemetrySettingsSchema>;
export const telemetryDefaults: TelemetrySettings = {
  level:'info', components:{}, console:{enabled:true}, file:{enabled:true,sizeBytes:10*1024*1024,frequency:'daily',historyCount:7},
  otlp:{enabled:false,protocol:'http/protobuf',endpoint:null,headersRef:null,timeoutMs:10_000,queueSize:2048,batchSize:512,delayMs:5000,sampleRatio:1,metricIntervalMs:60_000},
};
function field(path:string, effect:'hot'|'restart', consumerId:string) {
  return { labelKey:`telemetry${path.replaceAll('/','.')}`,descriptionKey:`telemetry${path.replaceAll('/','.')}.description`,scopes:['installation' as const],visibility:'advanced' as const,effect,consumerIds:[consumerId] };
}
export const telemetryDefinition: SettingsModule<TelemetrySettings> = {
  id:'platform.telemetry',definitionRevision:'telemetry-1',owner:'platform.telemetry',schema:TelemetrySettingsSchema,defaults:telemetryDefaults,
  fields:Object.fromEntries([
    ...['/level','/components'].map(path=>[path,field(path,'hot','telemetry.logger')]),
    ...['/console','/file'].map(path=>[path,field(path,'restart','telemetry.logger')]),
    ['/otlp',field('/otlp','restart','telemetry.otel')],
  ]),
};
