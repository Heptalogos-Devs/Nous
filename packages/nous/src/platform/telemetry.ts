import { NodeSDK } from '@opentelemetry/sdk-node';
import { trace,context,propagation,SpanStatusCode } from '@opentelemetry/api';
import pino from 'pino';
export function startTelemetry(installationId:string,level='info') {
  const sdk=new NodeSDK({serviceName:'nous-wave',...(process.env.OTEL_EXPORTER_OTLP_ENDPOINT?{}:{spanProcessors:[]})});sdk.start();
  const logger=pino({level,base:{service:'nous-wave',instanceId:installationId},redact:['req.headers.authorization','req.headers.cookie','password','apiKey','secret','credential','databaseUrl']});
  return {logger,sdk,async close(){await sdk.shutdown();}};
}
export async function span<T>(name:string,headers:Record<string,string>,fn:()=>Promise<T>) {
  const parent=propagation.extract(context.active(),headers);return trace.getTracer('nous-wave').startActiveSpan(name,{},parent,async active=>{try{return await fn();}catch(error){active.setStatus({code:SpanStatusCode.ERROR,message:error instanceof Error?error.message:String(error)});throw error;}finally{active.end();}});
}
