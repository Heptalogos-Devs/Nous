// Adapted from Nous-Wave platform/telemetry.ts: Pino + NodeSDK composition.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { once } from 'node:events';
import pino from 'pino';
import type { Logger, Level, StreamEntry } from 'pino';
import { context, propagation, trace, metrics, diag, DiagLogLevel, SpanStatusCode, isSpanContextValid } from '@opentelemetry/api';
import { logs } from '@opentelemetry/api-logs';
import type { CallContext, Problem } from '@nous-wave/contracts/common';
import type { Paths } from '../bootstrap/locations.js';
import type { Settings } from '../settings/index.js';
import { problem, ProblemError, safeProblem } from '../errors.js';
import { TelemetrySettingsSchema, type TelemetrySettings } from './config.js';
import { bindings, builtinEvents, dbosAdapter, logHook, operationName, problemFields } from './safe.js';
import { startOTel, type OTelOptions } from './otel.js';
export { telemetryDefinition, telemetryDefaults, TelemetrySettingsSchema, LogLevelSchema, type TelemetrySettings } from './config.js';
export { guardSpanExporter, guardMetricExporter } from './otel.js';
export type { OTelOptions } from './otel.js';

export interface TelemetryOptions extends Pick<OTelOptions,'headers'|'exporters'|'version'> {
  product:'nous'|'hept'; installationId:string; paths:Pick<Paths,'log'>;
  mode:'service'|'cli'; bootstrapLevel?:Level|'silent'; processBootId?:string;
  /** Fixed, code-owned event names. Unregistered free-form messages are discarded. */
  events?:readonly string[];
  onError?:(problem:Problem)=>void;
}
let ownsSDK=false;
const loggerPaths=['/level','/components','/console','/file'];
const hotPaths=['/level','/components'];
function timeout<T>(work:Promise<T>, milliseconds:number):Promise<T> {
  let timer:ReturnType<typeof setTimeout>;
  return Promise.race([work,new Promise<T>((_resolve,reject)=>{timer=setTimeout(()=>reject(problem('telemetry.shutdown_timeout','unavailable','遥测操作超时')),milliseconds);})]).finally(()=>clearTimeout(timer));
}

export function createTelemetry(options:TelemetryOptions) {
  const processBootId=options.processBootId??randomUUID();
  const consoleStream=options.mode==='cli'?process.stderr:process.stdout;
  const streams=pino.multistream([{stream:consoleStream,level:'trace'}]);
  const logger=pino({level:options.bootstrapLevel??'info',base:{product:options.product,installationId:options.installationId,processBootId},
    timestamp:pino.stdTimeFunctions.isoTime,
    formatters:{bindings},hooks:{logMethod:logHook(new Set([...builtinEvents,...options.events??[]]))},
    // Defence for a serializer supplied by an integration; logMethod already projects input.
    redact:{paths:['password','apiKey','secret','authorization','cookie','req.headers.authorization','req.headers.cookie'],remove:true},
  },streams);
  const components=new Map<string,Logger>();
  let config:TelemetrySettings|undefined;
  let file:ReturnType<typeof pino.transport>|undefined;
  let otel:ReturnType<typeof startOTel>|undefined;
  let sdkOwned=false;
  let diagOwned=false;
  let settingsOwner:Settings|undefined;
  let loggerConsumer:ReturnType<Settings['registerConsumer']>|undefined;
  let otelConsumer:ReturnType<Settings['registerConsumer']>|undefined;
  let unsubscribe:(()=>void)|undefined;
  let reload=Promise.resolve();
  let closing:Promise<void>|undefined;
  let closed=false;
  let sinkProblem:Problem|undefined;
  function component(name:string):Logger {
    if(!/^[a-zA-Z0-9_.:-]{1,128}$/.test(name))throw problem('telemetry.invalid_component','validation','日志组件标识无效');
    let child=components.get(name);
    if(!child){child=logger.child({component:name});if(config?.components[name])child.level=config.components[name];components.set(name,child);}
    return child;
  }
  function remove(stream:unknown) {
    // Pino's public multistream.remove(id) uses the ids present in its streams array.
    const entry=streams.streams.find(entry=>entry.stream===stream) as (StreamEntry & {id:number})|undefined;
    if(entry)streams.remove(entry.id);
  }
  function failure(code:string,category:Problem['category'],message:string):Problem {
    const value=problem(code,category,message).problem;options.onError?.(value);return value;
  }
  function applyLevel(next:TelemetrySettings) {
    logger.level=next.level;
    for(const [name,child]of components)child.level=next.components[name]??next.level;
    config=next;
  }
  async function startSinks(next:TelemetrySettings) {
    if(next.file.enabled){
      const directory=join(options.paths.log,'telemetry');
      try{await mkdir(directory,{recursive:true,mode:0o700});}catch(error){const code=error&&typeof error==='object'&&'code'in error?error.code:undefined;throw problem('telemetry.sink_failed',code==='ENOSPC'?'resource_pressure':code==='EACCES'?'forbidden':'unavailable','日志目录不可写');}
      // pino-roll numeric size is MB; explicit k units preserve our byte contract.
      const transport=pino.transport({target:'pino-roll',options:{file:join(directory,'events.log'),size:`${next.file.sizeBytes/1024}k`,frequency:next.file.frequency,mkdir:false,mode:0o600,limit:{count:next.file.historyCount,removeOtherLogFiles:true}}});
      file=transport;
      transport.on('error',()=>{remove(transport);sinkProblem=failure('telemetry.sink_failed','resource_pressure','日志文件不可写');if(!closed){logger.error({event:'telemetry.sink_failed',problem:sinkProblem});loggerConsumer?.reportFailure(sinkProblem,['/file']);}});
      await timeout(once(transport,'ready').then(()=>undefined),5000);
      streams.add({stream:transport,level:'trace'});
    }
    if(!next.console.enabled)remove(consoleStream);
  }
  function configureDiag() {
    const log=component('otel');
    diag.setLogger({error:()=>log.error({event:'otel.event',classification:'internal'}),warn:()=>log.warn({event:'otel.event',classification:'internal'}),info:()=>log.info({event:'otel.event',classification:'internal'}),debug:()=>log.debug({event:'otel.event',classification:'internal'}),verbose:()=>log.trace({event:'otel.event',classification:'internal'})},{logLevel:DiagLogLevel.ALL,suppressOverrideMessage:true});
    diagOwned=true;
  }
  async function refresh() {
    if(closed||!loggerConsumer)return;
    try{
      const snapshot=await loggerConsumer.snapshot();
      const next=TelemetrySettingsSchema.parse(snapshot.values);
      applyLevel(next);
      loggerConsumer.reportApplied(snapshot,hotPaths);
      if(sinkProblem)loggerConsumer.reportFailure(sinkProblem,['/file']);
      logger.debug({event:'telemetry.settings_applied'});
    }catch(error){const value=safeProblem(error,'telemetry.settings_failed');loggerConsumer.reportFailure(value);logger.error({event:'telemetry.settings_applied',problem:value});}
  }
  async function bindSettings(settings:Settings) {
    if(settingsOwner||closed)throw problem('telemetry.already_bound','conflict','遥测设置已绑定或实例已关闭');
    if(ownsSDK)throw problem('telemetry.provider_conflict','conflict','当前进程已有遥测SDK');
    ownsSDK=true;sdkOwned=true;
    settingsOwner=settings;
    const registration={processBootId,namespace:'platform.telemetry',scope:{kind:'installation' as const,installationId:options.installationId}};
    loggerConsumer=settings.registerConsumer({...registration,consumerId:'telemetry.logger'});
    otelConsumer=settings.registerConsumer({...registration,consumerId:'telemetry.otel'});
    try{
      const snapshot=await loggerConsumer.snapshot();
      const next=TelemetrySettingsSchema.parse(snapshot.values);
      applyLevel(next);
      loggerConsumer.reportApplied(snapshot,hotPaths);
      await startSinks(next);
      loggerConsumer.reportApplied(snapshot,loggerPaths);
      configureDiag();
      otel=startOTel(next.otlp,{...options,processBootId,diagnostics:configureDiag});
      otelConsumer.reportApplied(snapshot,['/otlp']);
      unsubscribe=settings.subscribe(event=>{
        if(event.kind==='resync'||(event.change.namespace==='platform.telemetry'&&event.change.scope.kind==='installation'&&event.change.scope.installationId===options.installationId))reload=reload.then(refresh);
      });
      logger.info({event:'telemetry.ready'});
    }catch(error){const value=safeProblem(error,'telemetry.start_failed');loggerConsumer.reportFailure(value);otelConsumer.reportFailure(value);throw new ProblemError(value);}
  }
  async function flush(milliseconds=10_000) {
    try{await timeout(Promise.all([
      new Promise<void>((resolve,reject)=>{logger.flush(error=>error?reject(problem('telemetry.flush_failed','unavailable','日志刷新失败')):resolve());}),
      new Promise<void>(resolve=>{consoleStream.write('',()=>resolve());}),
      ...(file?[new Promise<void>((resolve,reject)=>file!.flush(error=>error?reject(problem('telemetry.flush_failed','unavailable','日志刷新失败')):resolve()))]:[]),
      otel?.flush()??Promise.resolve(),
    ]).then(()=>undefined),milliseconds);}catch(error){if(error instanceof ProblemError)throw error;throw problem('telemetry.flush_failed','unavailable','日志刷新失败');}
  }
  function close(milliseconds=10_000):Promise<void> {
    return closing??=(async()=>{
      closed=true;unsubscribe?.();await reload;
      logger.info({event:'telemetry.closing'});
      let error:unknown;
      try{await flush(milliseconds);}catch(value){error=value;}
      const shutdown=[otel?.close()??Promise.resolve()];
      if(file){const transport=file;shutdown.push(once(transport,'close').then(()=>undefined));try{transport.end();}catch{error??=problem('telemetry.close_failed','unavailable','日志关闭失败');}}
      try{await timeout(Promise.all(shutdown),milliseconds);}catch(value){error??=value;}
      loggerConsumer?.close();otelConsumer?.close();logger.level='silent';
      if(sdkOwned){ownsSDK=false;trace.disable();metrics.disable();logs.disable();context.disable();propagation.disable();}
      if(diagOwned)diag.disable();
      if(error)throw problem('telemetry.close_failed','unavailable','遥测关闭未完成');
    })();
  }
  return {
    logger,dbosLogger:dbosAdapter(component('dbos')),processBootId,component,bindSettings,
    reloadSettings:()=>reload=reload.then(refresh),flush,close,
    forContext(ctx:CallContext,name:string){return component(name).child(bindings({subjectId:ctx.subjectId,consumerId:ctx.consumer.id,...ctx.trace}));},
    async span<T>(ctx:CallContext,name:string,fn:(traced:CallContext)=>Promise<T>):Promise<T>{
      const carrier=ctx.trace.traceparent?{traceparent:ctx.trace.traceparent}:{};
      const parent=propagation.extract(context.active(),carrier);
      return trace.getTracer(options.product).startActiveSpan(operationName(name),{attributes:{'app.product':options.product,'app.installation.id':options.installationId,'app.operation.id':ctx.trace.operationId,...(ctx.subjectId?{'app.subject.id':ctx.subjectId}:{})}},parent,async active=>{
        const spanContext=active.spanContext();
        const outgoing:Record<string,string>={};propagation.inject(context.active(),outgoing);
        const traced=isSpanContextValid(spanContext)?{...ctx,trace:{...ctx.trace,traceId:spanContext.traceId,traceparent:outgoing.traceparent}}:ctx;
        try{return await fn(traced);}catch(error){active.setStatus({code:SpanStatusCode.ERROR});const safe=problemFields(error);active.addEvent('operation.failed',{'problem.code':typeof safe?.code==='string'?safe.code:'system.failed','problem.category':typeof safe?.category==='string'?safe.category:'failed'});throw error;}finally{active.end();}
      });
    },
  };
}
export type Telemetry=ReturnType<typeof createTelemetry>;
