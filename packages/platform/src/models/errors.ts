// The SDK's official error identity/retry classification is retained; raw messages
// and request/response bodies never enter default DBOS error checkpoints.
import {APICallError} from '@ai-sdk/provider';
import {NoObjectGeneratedError} from 'ai';
import {ProblemError,problem,safeProblem} from '../errors.js';
import type {Problem} from '@nous-wave/contracts/common';
export function providerError(error:unknown):Error {
 if(NoObjectGeneratedError.isInstance(error))return problem('models.output_invalid','validation','模型实际输出不符合领域结构合同');
 if(APICallError.isInstance(error)){
  const status=typeof error.statusCode==='number'&&Number.isInteger(error.statusCode)&&error.statusCode>=100&&error.statusCode<=599?error.statusCode:undefined;
  const headers:Record<string,string>={};
  for(const name of ['x-request-id','request-id','retry-after']){const value=error.responseHeaders?.[name];if(typeof value==='string'&&value.length<256&&/^[\x20-\x7e]+$/.test(value))headers[name]=value;}
  return new APICallError({message:'模型提供方调用失败',url:'',requestBodyValues:{},statusCode:status,isRetryable:error.isRetryable===true,responseHeaders:headers});
 }
 if(error instanceof ProblemError)return new ProblemError(error.problem);
 if(error instanceof DOMException&&error.name==='AbortError')return new DOMException('模型调用已取消','AbortError');
 return problem('models.provider_failed','unavailable','模型提供方调用失败');
}
export function providerProblem(error:unknown):Problem {
 if(APICallError.isInstance(error))return problem('models.provider_failed','unavailable','模型提供方调用失败',{details:{...(error.statusCode?{providerStatus:error.statusCode}:{}),isRetryable:error.isRetryable,...(error.responseHeaders?.['x-request-id']?{requestId:error.responseHeaders['x-request-id']}:{})}}).problem;
 return safeProblem(providerError(error),'models.provider_failed');
}
/** A transform does not see rejected reader.read() or cancel(); guard those too. */
export function safeProviderStream<T>(source:ReadableStream<T>):ReadableStream<T>{
 const reader=source.getReader();let ended=false;
 return new ReadableStream<T>({
  async pull(controller){try{const next=await reader.read();if(next.done){ended=true;reader.releaseLock();controller.close();return;}const chunk=next.value;if(chunk&&typeof chunk==='object'&&'type'in chunk&&chunk.type==='error'&&'error'in chunk)controller.enqueue({...chunk,error:providerError(chunk.error)});else controller.enqueue(chunk);}catch(error){ended=true;try{reader.releaseLock();}catch{}controller.error(providerError(error));}},
  async cancel(){if(ended)return;ended=true;try{await reader.cancel('cancelled');}catch(error){throw providerError(error);}finally{reader.releaseLock();}},
 });
}
