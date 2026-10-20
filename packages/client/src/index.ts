import type { QueryInput,QueryResult,ContextPrepareInput,ContextView } from '@nous-wave/contracts/cognition';
import type { MaterialView } from '@nous-wave/contracts/materials';
export class NousClient {
  readonly baseURL:string;
  constructor(private options:{baseURL:string;apiKey?:string;fetch?:typeof fetch}) {this.baseURL=options.baseURL.replace(/\/$/,'');}
  async request<T>(method:string,path:string,body?:unknown,options:{submissionId?:string;traceparent?:string;purpose?:string;audience?:string[]}={}):Promise<T> {
    const headers:Record<string,string>={accept:'application/json'};
    if(body!==undefined)headers['content-type']='application/json';
    if(this.options.apiKey)headers.authorization=`Bearer ${this.options.apiKey}`;
    if(options.submissionId)headers['idempotency-key']=options.submissionId;
    if(options.traceparent)headers.traceparent=options.traceparent;
    if(options.purpose)headers['x-purpose']=options.purpose;
    if(options.audience)headers['x-audience']=JSON.stringify(options.audience);
    const response=await (this.options.fetch??fetch)(`${this.baseURL}${path}`,{method,headers,...(body!==undefined?{body:JSON.stringify(body)}:{})});
    const result=await response.json();if(!response.ok)throw Object.assign(new Error(result.detail??result.title??`Nous HTTP ${response.status}`),result);return result as T;
  }
  query(subjectId:string,input:QueryInput){return this.request<QueryResult>('POST',`/v1/subjects/${subjectId}/knowledge/query`,input);}
  prepare(subjectId:string,input:ContextPrepareInput){return this.request<ContextView>('POST',`/v1/subjects/${subjectId}/context-views`,input);}
  capture(subjectId:string,input:unknown){return this.request<MaterialView>('POST',`/v1/subjects/${subjectId}/materials`,input);}
  async uploadBlob(subjectId:string,bytes:Uint8Array|Blob,mediaType:string,options:{submissionId?:string;traceparent?:string;purpose?:string;audience?:string[]}={}) {
    const headers:Record<string,string>={'content-type':'application/octet-stream','x-content-media-type':mediaType,accept:'application/json'};
    if(this.options.apiKey)headers.authorization=`Bearer ${this.options.apiKey}`;
    if(options.submissionId)headers['idempotency-key']=options.submissionId;
    if(options.traceparent)headers.traceparent=options.traceparent;
    if(options.purpose)headers['x-purpose']=options.purpose;
    if(options.audience)headers['x-audience']=JSON.stringify(options.audience);
    const body=bytes instanceof Blob?bytes:new Blob([Uint8Array.from(bytes)],{type:mediaType});
    const response=await(this.options.fetch??fetch)(`${this.baseURL}/v1/subjects/${subjectId}/blobs`,{method:'POST',headers,body});
    const result=await response.json();if(!response.ok)throw Object.assign(new Error(result.detail??result.title??`Nous HTTP ${response.status}`),result);
    return result as {ref:import('@nous-wave/contracts/common').Ref;digest:string;bytes:number};
  }
}
export { Configuration,DefaultApi } from './generated/index.js';
