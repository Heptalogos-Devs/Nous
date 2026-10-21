// Thin projections around the standard OTel exporters; batching/retry remain in OTel.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { NodeSDK } from '@opentelemetry/sdk-node';
import { BatchSpanProcessor, NoopSpanProcessor, ParentBasedSampler, TraceIdRatioBasedSampler } from '@opentelemetry/sdk-trace-base';
import type { ReadableSpan, SpanExporter, SpanProcessor } from '@opentelemetry/sdk-trace-base';
import { AggregationType, MetricReader, PeriodicExportingMetricReader } from '@opentelemetry/sdk-metrics';
import type { IMetricReader, PushMetricExporter, ResourceMetrics } from '@opentelemetry/sdk-metrics';
import { resourceFromAttributes } from '@opentelemetry/resources';
import type { Resource } from '@opentelemetry/resources';
import { ExportResultCode, W3CTraceContextPropagator } from '@opentelemetry/core';
import type { ExportResult } from '@opentelemetry/core';
import { OTLPTraceExporter as ProtobufTrace } from '@opentelemetry/exporter-trace-otlp-proto';
import { OTLPTraceExporter as JsonTrace } from '@opentelemetry/exporter-trace-otlp-http';
import { OTLPTraceExporter as GrpcTrace } from '@opentelemetry/exporter-trace-otlp-grpc';
import { OTLPMetricExporter as ProtobufMetric } from '@opentelemetry/exporter-metrics-otlp-proto';
import { OTLPMetricExporter as JsonMetric } from '@opentelemetry/exporter-metrics-otlp-http';
import { OTLPMetricExporter as GrpcMetric } from '@opentelemetry/exporter-metrics-otlp-grpc';
import { credentials, Metadata } from '@grpc/grpc-js';
import { problem } from '../errors.js';
import { attributes, operationName } from './safe.js';
import type { TelemetrySettings } from './config.js';

function result(callback:(result:ExportResult)=>void) {
  return (value:ExportResult)=>callback(value.code===ExportResultCode.SUCCESS?{code:ExportResultCode.SUCCESS}:{code:ExportResultCode.FAILED,error:problem('telemetry.export_failed','unavailable','遥测导出失败')});
}
function projectSpan(span:ReadableSpan, resource:Resource):ReadableSpan {
  return {
    name:operationName(span.name), resource, spanContext:()=>span.spanContext(),kind:span.kind,
    parentSpanContext:span.parentSpanContext,startTime:span.startTime,endTime:span.endTime,duration:span.duration,ended:span.ended,
    droppedAttributesCount:span.droppedAttributesCount,droppedEventsCount:span.droppedEventsCount,droppedLinksCount:span.droppedLinksCount,
    status:{code:span.status.code}, attributes:attributes(span.attributes),
    instrumentationScope:{name:operationName(span.instrumentationScope.name),...(span.instrumentationScope.version?{version:operationName(span.instrumentationScope.version)}:{})},
    links:span.links.map(link=>({...link,attributes:attributes(link.attributes??{})})),
    events:span.events.map(event=>({time:event.time,name:event.name==='exception'||/^Step attempt \d+ failed$/.test(event.name)?event.name:operationName(event.name),attributes:attributes(event.attributes??{}),droppedAttributesCount:event.droppedAttributesCount})),
  };
}
/** Exporters receive safe standard ReadableSpan values, never raw status/exception text. */
export function guardSpanExporter(inner:SpanExporter, resource:Resource):SpanExporter {
  return {
    export(spans,callback){try{inner.export(spans.map(span=>projectSpan(span,resource)),result(callback));}catch{result(callback)({code:ExportResultCode.FAILED});}},
    async shutdown(){try{await inner.shutdown();}catch{throw problem('telemetry.export_failed','unavailable','遥测导出关闭失败');}},
    async forceFlush(){try{await inner.forceFlush?.();}catch{throw problem('telemetry.export_failed','unavailable','遥测导出刷新失败');}},
  };
}
export function guardMetricExporter(inner:PushMetricExporter, resource:Resource):PushMetricExporter {
  return {
    export(metrics,callback){
      const safe:ResourceMetrics={resource,scopeMetrics:metrics.scopeMetrics.map(scope=>({
        scope:{name:operationName(scope.scope.name),...(scope.scope.version?{version:operationName(scope.scope.version)}:{})},
        metrics:scope.metrics.map(metric=>({...metric,descriptor:{...metric.descriptor,name:operationName(metric.descriptor.name),description:''},dataPoints:metric.dataPoints.map(point=>({...point,attributes:attributes(point.attributes,true)}))})) as typeof scope.metrics,
      }))};
      try{inner.export(safe,result(callback));}catch{result(callback)({code:ExportResultCode.FAILED});}
    },
    selectAggregationTemporality:inner.selectAggregationTemporality?.bind(inner),selectAggregation:inner.selectAggregation?.bind(inner),
    async forceFlush(){try{await inner.forceFlush();}catch{throw problem('telemetry.export_failed','unavailable','遥测导出刷新失败');}},
    async shutdown(){try{await inner.shutdown();}catch{throw problem('telemetry.export_failed','unavailable','遥测导出关闭失败');}},
  };
}
export interface OTelOptions {
  product:string; installationId:string; processBootId:string; version?:string;
  headers?: (ref:string)=>Promise<Record<string,string>>;
  /** Optional official exporter injection for a collector/transport owned by the product host. */
  exporters?: (config:TelemetrySettings['otlp'])=>{traces:SpanExporter;metrics:PushMetricExporter};
  diagnostics?:()=>void;
}
class DisabledMetricReader extends MetricReader {
  constructor(){super({aggregationSelector:()=>({type:AggregationType.DROP})});}
  protected async onShutdown(){}
  protected async onForceFlush(){}
}
export function startOTel(config:TelemetrySettings['otlp'], options:OTelOptions) {
  if(process.env.OTEL_SDK_DISABLED?.toLowerCase()==='true')throw problem('telemetry.environment_conflict','validation','OTEL_SDK_DISABLED与安装遥测设置冲突');
  for(const variable of ['OTEL_LOG_LEVEL','OTEL_EXPORTER_OTLP_HEADERS','OTEL_EXPORTER_OTLP_TRACES_HEADERS','OTEL_EXPORTER_OTLP_METRICS_HEADERS']) {
    if(process.env[variable])throw problem('telemetry.environment_conflict','validation','遥测等级与身份需使用安装配置', {args:{variable}});
  }
  const resource=resourceFromAttributes({'service.name':options.product,'service.instance.id':options.installationId,'service.version':options.version??'unknown','app.process_boot_id':options.processBootId});
  let spanProcessors:SpanProcessor[]=[new NoopSpanProcessor()];
  let metricReaders:IMetricReader[]=[new DisabledMetricReader()];
  if(config.enabled){
    const headers=async()=>{
      if(!config.headersRef)return {};
      if(!options.headers)throw problem('telemetry.headers_unavailable','unavailable','OTLP身份引用未绑定');
      try{return await options.headers(config.headersRef);}catch{throw problem('telemetry.headers_unavailable','forbidden','OTLP身份不可用');}
    };
    let exporters=options.exporters?.(config);
    if(!exporters){
      if(config.protocol==='grpc'){
        const channel=config.endpoint!.startsWith('http:')?credentials.createInsecure():credentials.createSsl();
        const call=credentials.createFromMetadataGenerator((_parameters,callback)=>{void headers().then(value=>{const metadata=new Metadata();for(const [key,item]of Object.entries(value))metadata.set(key,item);callback(null,metadata);}).catch(()=>callback(new Error('OTLP身份不可用')));});
        const auth=config.headersRef?credentials.combineChannelCredentials(channel,call):channel;
        exporters={traces:new GrpcTrace({url:config.endpoint!,credentials:auth,timeoutMillis:config.timeoutMs}),metrics:new GrpcMetric({url:config.endpoint!,credentials:auth,timeoutMillis:config.timeoutMs})};
      }else{
        const options={url:config.endpoint!,timeoutMillis:config.timeoutMs,headers};
        const traceOptions={...options,url:new URL('v1/traces',config.endpoint!.replace(/\/?$/,'/')).href};
        const metricOptions={...options,url:new URL('v1/metrics',config.endpoint!.replace(/\/?$/,'/')).href};
        exporters=config.protocol==='http/protobuf'?{traces:new ProtobufTrace(traceOptions),metrics:new ProtobufMetric(metricOptions)}:{traces:new JsonTrace(traceOptions),metrics:new JsonMetric(metricOptions)};
      }
    }
    spanProcessors=[new BatchSpanProcessor(guardSpanExporter(exporters.traces,resource),{maxQueueSize:config.queueSize,maxExportBatchSize:config.batchSize,scheduledDelayMillis:config.delayMs,exportTimeoutMillis:config.timeoutMs})];
    metricReaders=[new PeriodicExportingMetricReader({exporter:guardMetricExporter(exporters.metrics,resource),exportIntervalMillis:config.metricIntervalMs,exportTimeoutMillis:Math.min(config.timeoutMs,config.metricIntervalMs)})];
  }
  // Explicit arrays and disabled detection prevent SDK environment auto-export and command args.
  // A native no-op processor keeps local trace ids; the DROP reader blocks env fallback export.
  const sdk=new NodeSDK({resource,autoDetectResources:false,resourceDetectors:[],instrumentations:[],spanProcessors,metricReaders,logRecordProcessors:[],textMapPropagator:new W3CTraceContextPropagator(),sampler:new ParentBasedSampler({root:new TraceIdRatioBasedSampler(config.sampleRatio)})});
  // NodeSDK reads OTEL_LOG_LEVEL in its constructor; restore our safe official diag logger.
  options.diagnostics?.();
  sdk.start();
  return {sdk,async flush(){await Promise.all([...spanProcessors.map(processor=>processor.forceFlush()),...metricReaders.map(reader=>reader.forceFlush())]);},async close(){await sdk.shutdown();}};
}
