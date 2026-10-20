import { DBOS } from '@dbos-inc/dbos-sdk';
import { durableCalls } from '@dbos-inc/vercel-ai';
import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { generateText, wrapLanguageModel, Output, APICallError, NoObjectGeneratedError, type LanguageModelMiddleware, type LanguageModel } from 'ai';
import type { z } from 'zod';
import type { CallContext,Ref } from '@nous-wave/contracts/common';
import type { ContextPrepareInput,ContextView } from '@nous-wave/contracts/cognition';
import type { MaterialView } from '@nous-wave/contracts/materials';
import type { ModelResource,Settings } from '@nous-wave/contracts/management';
import type { createInvocations } from '../management/invocations.js';
import { authorize } from './security.js';
import { fail } from './errors.js';
import { now } from './context.js';
export interface ModelsDependencies {
  installationId:string;
  settings:{read(ctx:CallContext,role?:string):Promise<{effective:Settings}>};
  models:{read(ctx:CallContext,id:string):Promise<ModelResource>};
  secrets:{resolve(ref:string):Promise<string>};
  invocations:ReturnType<typeof createInvocations>;
  materials:{capture(ctx:CallContext,input:unknown):Promise<MaterialView>};
  encounters:{accept(ctx:CallContext,input:unknown):Promise<unknown>};
  prepare(ctx:CallContext,input:ContextPrepareInput):Promise<ContextView>;
  provider?:(resource:ModelResource,credential:string)=>Exclude<LanguageModel,string>;
}
export function createModels(deps:ModelsDependencies) {
  return {
    async generate(ctx:CallContext,input:{role:string;prompt:string;schema?:z.ZodType;inputRefs?:Ref[]}) {
      authorize(ctx,'models.invoke');
      const configure=async()=>{const settings=(await deps.settings.read(ctx,input.role)).effective;const profile=settings.roles[input.role];if(!profile)fail('capability_unavailable',`尚未配置 ${input.role} 模型`);const resource=await deps.models.read(ctx,profile.modelResourceId);return {profile,resource,invocationId:`${DBOS.workflowID??ctx.traceContext.operationId}:model:${DBOS.stepID??0}`};};
      const {profile,resource,invocationId}=DBOS.isWithinWorkflow()?await DBOS.runStep(configure,{name:'models.configure',retriesAllowed:false}):await configure();
      const maximumOutput=Math.min(profile.maxOutputTokens,resource.outputLimit);
      let currentView:ContextView|undefined;
      let currentId=`${invocationId}:0`;
      const consumer=ctx.actor.consumer??{kind:'worker' as const,host:deps.installationId,id:`${ctx.subjectId}:${input.role}`};
      ctx={...ctx,consumer};
      const guard:LanguageModelMiddleware={specificationVersion:'v4',wrapGenerate:async({doGenerate,params})=>{
        await deps.invocations.prepare(ctx,{invocationId:currentId,role:input.role,modelResourceId:resource.id,inputTokensEstimate:Buffer.byteLength(JSON.stringify({prompt:params.prompt,tools:params.tools,responseFormat:params.responseFormat}),'utf8'),maxOutputTokens:maximumOutput,inputRefs:currentView?.inputPortions.map(p=>p.ref)??input.inputRefs??[]});
        await deps.invocations.dispatch(ctx,currentId);
        try{return await doGenerate();}catch(error){
          const rejected=APICallError.isInstance(error)&&[400,401,403,404,422,429].includes(error.statusCode??0);
          await deps.invocations.complete(ctx,currentId,{status:rejected?'not_sent':'unknown'});throw error;
        }
      }};
      const acceptance:LanguageModelMiddleware={specificationVersion:'v4',wrapGenerate:async({doGenerate})=>{
        const raw=await doGenerate();
        const settle=()=>deps.invocations.complete(ctx,currentId,{status:'completed',inputTokens:raw.usage.inputTokens.total,outputTokens:raw.usage.outputTokens.total,cacheReadTokens:raw.usage.inputTokens.cacheRead,cacheWriteTokens:raw.usage.inputTokens.cacheWrite,providerRequestId:raw.response?.id});
        if(DBOS.isWithinWorkflow())await DBOS.runStep(settle,{name:'models.settle',retriesAllowed:false});else await settle();
        const output=await deps.materials.capture(ctx,{kind:'model_output',mediaType:'application/json',text:JSON.stringify(raw.content),source:{inputs:currentView?.inputPortions.map(p=>p.ref)??input.inputRefs??[],roots:[]}});
        await deps.encounters.accept(ctx,{consumer,invocationId:currentId,step:0,contextViewRef:currentView?.ref,portions:currentView?.inputPortions.map(p=>({ref:{...p.ref,selector:p.selector??p.ref.selector}}))??[],resultRef:output.ref,observed:currentView?.observed??now(),purpose:ctx.purpose});
        return raw;
      }};
      const credential=await deps.secrets.resolve(resource.credentialRef);
      const provider=deps.provider?.(resource,credential)??createOpenAICompatible({name:resource.id,baseURL:resource.baseURL,apiKey:credential,supportsStructuredOutputs:resource.capabilities.includes('structured')}).chatModel(resource.modelId);
      const guarded=wrapLanguageModel({model:provider,middleware:guard});
      const durable=wrapLanguageModel({model:guarded,middleware:durableCalls({retriesAllowed:false,name:`models.${input.role}`})});
      const model=wrapLanguageModel({model:durable,middleware:acceptance});
      const result=await generateText({model,instructions:'按当前角色的任务与输入作答。只引用实际提供的来源。',prompt:input.prompt,maxOutputTokens:maximumOutput,maxRetries:0,temperature:profile.temperature,
        prepareStep:async step=>{
          currentId=`${invocationId}:${step.stepNumber}`;
          const material=await deps.materials.capture(ctx,{kind:'model_input',mediaType:'application/json',text:JSON.stringify(step.messages),source:{inputs:input.inputRefs??[],roots:[]}});
          currentView=await deps.prepare(ctx,{consumer,role:input.role,roleTask:'根据实际输入完成本次认知工作。',latestMaterials:[{ref:material.ref,text:material.text??'',required:true}],model:{resourceId:resource.id,modelId:resource.modelId,contextLimit:resource.contextLimit},budget:{outputTokens:maximumOutput},observed:now()});
          return {instructions:currentView.blocks.map(b=>b.text).join('\n\n'),messages:[{role:'user' as const,content:'完成本次职责，保持约定的输出结构。'}]};
        },
        ...(input.schema?{output:Output.object({schema:input.schema})}:{}),experimental_telemetry:{isEnabled:true,recordInputs:false,recordOutputs:false}}).catch(error=>{if(NoObjectGeneratedError.isInstance(error))fail('invalid_input',`模型输出不符合本次结构，请查看调用 ${invocationId} 的已保存成果`,{operationId:ctx.traceContext.operationId});throw error;});
      return {output:input.schema?result.output:result.text,text:result.text,invocationId,usage:result.totalUsage};
    },
  };
}
