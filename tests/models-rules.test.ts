import assert from 'node:assert/strict';
import {test} from 'node:test';
import {createRequire} from 'node:module';
import {PriceBookSchema} from '@nous-wave/contracts/models';
import {languageUsage,modelCost,reserveCost} from '../packages/platform/src/models/usage.js';
import {providerError,safeProviderStream} from '../packages/platform/src/models/errors.js';
const {APICallError}=createRequire(new URL('../packages/platform/package.json',import.meta.url))('@ai-sdk/provider');

test('总输入与缓存分项按真实单位计价，预留覆盖缓存的最贵路径',()=>{
 const usage=languageUsage({inputTokens:{total:100,noCache:60,cacheRead:30,cacheWrite:10},outputTokens:{total:20,text:15,reasoning:5}});
 const price=PriceBookSchema.parse({id:'cache-aware',revision:1,rates:[
  {unit:'uncached_input_tokens',amount:{amount:'0.6',currency:'USD'},per:'1000000'},
  {unit:'cache_read_tokens',amount:{amount:'0.06',currency:'USD'},per:'1000000'},
  {unit:'cache_write_tokens',amount:{amount:'0.75',currency:'USD'},per:'1000000'},
  {unit:'output_tokens',amount:{amount:'2.4',currency:'USD'},per:'1000000'},
 ]});
 assert.deepEqual(modelCost(price,usage),{amount:'0.0000933',currency:'USD'});
 assert.deepEqual(reserveCost(price,{value:100,method:'fixture-tokenizer'},20),{amount:'0.000123',currency:'USD'});
 assert.equal(modelCost(price,{inputTokens:100,outputTokens:20}),null);
 assert.equal(PriceBookSchema.safeParse({...price,rates:[...price.rates,{unit:'input_tokens',amount:{amount:'1',currency:'USD'},per:'1000000'}]}).success,false);
});

test('Provider异常保留官方重试分类，错误流的read拒绝和取消不传播正文',async()=>{
 const raw=new APICallError({message:'private-request-body',url:'https://example.test/?api_key=secret',requestBodyValues:{prompt:'private-prompt'},responseBody:'private-response',statusCode:429,isRetryable:true,responseHeaders:{'x-request-id':'request-1',authorization:'Bearer secret'}});
 const safe=providerError(raw) as Error&{statusCode?:number;isRetryable?:boolean};assert.ok(APICallError.isInstance(safe));assert.equal(safe.statusCode,429);assert.equal(safe.isRetryable,true);assert.equal(JSON.stringify(safe).includes('private'),false);assert.equal(JSON.stringify(safe).includes('secret'),false);
 const failed=safeProviderStream(new ReadableStream({pull(){throw raw;}})).getReader();await assert.rejects(failed.read(),error=>APICallError.isInstance(error)&&(error as {statusCode?:number}).statusCode===429&&!JSON.stringify(error).includes('private'));
 const cancelled=safeProviderStream(new ReadableStream({cancel(){throw raw;}})).getReader();await assert.rejects(cancelled.cancel('private-cancel-reason'),error=>APICallError.isInstance(error)&&!JSON.stringify(error).includes('private'));
});
