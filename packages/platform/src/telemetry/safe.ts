// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { ProblemSchema } from '@nous-wave/contracts/common';
import type { Bindings, Logger, LogFn, Level } from 'pino';
import type { DLogger, ContextualMetadata, StackTrace } from '@dbos-inc/dbos-sdk';
import { trace, type Attributes } from '@opentelemetry/api';
const identifier = /^[a-zA-Z0-9_.:/-]{1,512}$/;
const symbolic = /^[a-zA-Z0-9_.:-]{1,128}$/;
const bindingFields = new Set(['product','installationId','processBootId','component','traceId','spanId','operationId','workflowId','subjectId','consumerId','invocationId','taskId','effectId','modelResourceId','role','resourceId']);
const numberFields = new Set(['durationMs','bytes','count','statusCode','attempt','queueDepth','rssBytes','heapBytes','currentRevision']);
export const builtinEvents = ['log.event','log.unstructured','dbos.event','otel.event','telemetry.ready','telemetry.settings_applied','telemetry.sink_failed','telemetry.export_failed','telemetry.closing'];
export function problemFields(value: unknown): Record<string, unknown> | undefined {
  const candidate = value && typeof value === 'object' && 'problem' in value ? value.problem : value;
  const parsed = ProblemSchema.safeParse(candidate);
  if (!parsed.success) return undefined;
  const {code,category,status,messageKey,operationId,traceId,currentRevision,retry} = parsed.data;
  return { code,category,status,messageKey, ...(operationId ? {operationId}:{}),...(traceId ? {traceId}:{}),...(currentRevision === undefined ? {}:{currentRevision}),...(retry ? {retry:{mode:retry.mode}}:{}) };
}
export function bindings(value: Bindings): Bindings {
  return Object.fromEntries(Object.entries(value).filter(([key,item])=>bindingFields.has(key)&&typeof item==='string'&&identifier.test(item)));
}
export function safeRecord(value: unknown, events: ReadonlySet<string>): Record<string, unknown> {
  if (!value || typeof value !== 'object') return {event:'log.unstructured'};
  const object = value as Record<string,unknown>;
  const result: Record<string,unknown> = { ...bindings(object), event:typeof object.event==='string'&&events.has(object.event)?object.event:'log.event' };
  for (const [key,item] of Object.entries(object)) {
    if (numberFields.has(key)&&typeof item==='number'&&Number.isFinite(item)) result[key]=item;
    if (['category','result','classification'].includes(key)&&typeof item==='string'&&symbolic.test(item)) result[key]=item;
  }
  const error = problemFields(object.problem ?? object.err ?? object.error ?? value);
  if (error) result.problem=error;
  else if ((value instanceof Error || object.err || object.error)&&!result.classification) result.classification='unknown_error';
  return result;
}
export function logHook(events: ReadonlySet<string>) {
  return function (this: Logger, args: unknown[], method: LogFn) {
    const active=trace.getActiveSpan()?.spanContext();
    method.call(this, {...safeRecord(args[0],events),...(active?{traceId:active.traceId,spanId:active.spanId}:{})});
  };
}
export function dbosAdapter(logger: Logger): DLogger {
  function fields(input: unknown, metadata?: ContextualMetadata & StackTrace) {
    const attributes=metadata?.span?.attributes ?? {};
    const context:Record<string,unknown>={};
    for (const [source,target] of [['operationUUID','workflowId'],['dbos.operation.workflow_id','workflowId'],['operationName','resourceId'],['dbos.operation.name','resourceId']] as const) if(typeof attributes[source]==='string')context[target]=attributes[source];
    return {event:'dbos.event',...bindings(context),error:metadata?.error??input,classification:'internal'};
  }
  return {info:(value,metadata)=>logger.info(fields(value,metadata)),debug:(value,metadata)=>logger.debug(fields(value,metadata)),warn:(value,metadata)=>logger.warn(fields(value,metadata)),error:(value,metadata)=>logger.error(fields(value,metadata))};
}
const traceFields = new Set(['app.product','app.installation.id','app.component','app.operation','app.operation.id','app.subject.id','app.consumer.id','app.invocation.id','app.task.id','app.effect.id','problem.code','problem.category','operationUUID','operationName','operationType','applicationID','applicationVersion','executorID','queueName','dbos.operation.workflow_id','dbos.operation.name','dbos.operation.type','dbos.application.id','dbos.application.version','dbos.executor.id','dbos.queue.name','retryIntervalSeconds','ai.model.id','ai.model.provider','ai.operationId','ai.usage.inputTokens','ai.usage.outputTokens']);
const metricFields = new Set(['component','operation','modelFamily','result','category']);
export function attributes(value: Attributes, metric=false): Attributes {
  return Object.fromEntries(Object.entries(value).filter(([key,item]) => (metric?metricFields:traceFields).has(key) && ((typeof item==='string'&&identifier.test(item)) || (typeof item==='number'&&Number.isFinite(item)) || typeof item==='boolean')));
}
export function operationName(value: string): string { return symbolic.test(value)?value:'operation'; }
