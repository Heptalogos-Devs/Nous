// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Deterministic Host owner for the real pinned Gateway/worker/announcement/sender fixture.
import {appendFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import {retryNousSubagentCompletion} from 'openclaw/plugin-sdk/core';
const proof=process.env.NOUS_COMPLETION_FIXTURE_PROOF;
const parent=process.env.NOUS_COMPLETION_FIXTURE_PARENT;
const activity='11111111-2222-4333-8444-555555555555';
const log=(event,fields={})=>appendFileSync(proof,JSON.stringify({event,...fields})+'\n');
const handoffs=new Map(),registered=new Map([[parent,{activityRef:activity}]]),admitted=new Set();
let released=false,counter=0;
const engine={
 info:{id:'nous-cognition',name:'Native completion fixture',version:'1',ownsCompaction:false,managedModelInputRequired:true,transcriptPersistence:'host'},
 async ingest(){return {ingested:false}},async assemble(p){return {messages:p.messages,estimatedTokens:0}},async compact(){throw Error('Fixture must not compact')},
 async prepareSubagentSpawn(p){if(!registered.has(p.parentRunId))throw Error('Unregistered parent');handoffs.set(p.childSessionKey,p.parentRunId);await p.persistSessionBinding({parentNativeRunId:p.parentRunId,parentNativeSessionKey:p.parentSessionKey});log('handoff',{parentRunId:p.parentRunId,childSessionKey:p.childSessionKey});return {async rollback(){}}},
 async prepareSubagentCompletion(p){const original=handoffs.get(p.childSessionKey);if(!original || (p.parentRunId && p.parentRunId!==original))throw Error('Exact child handoff unavailable');if(p.continuation)throw Error('Completion cannot create a parent invocation');log('terminal',{...p});return {parentRunId:original,continuationAdmitted:false,resultRetained:true}},
 async prepareModelInput(p){if(!registered.has(p.runId)){const original=handoffs.get(p.sessionKey);if(!original)throw Error('Unregistered actual input');registered.set(p.runId,{activityRef:activity,parentRunId:original});}admitted.add(p.runId);log('admitted',{runId:p.runId,prompt:p.prompt,sessionKey:p.sessionKey});return {context:p.context,invocationId:p.runId+':fixture:'+ ++counter,applied:{projectionId:randomUUID(),sourceRefs:[]}}},
 async authorizeToolExecution(){return {async run(fn){return await fn()}}},
 captureNativeEvent(p,m,a,o){if(m.role==='user')return;m.nousHostBridge={identity:{version:1,contactRequestId:randomUUID(),subjectId:'11111111-1111-4111-8111-111111111111',nousSessionId:'22222222-2222-4222-8222-222222222222',invocationId:a?.invocationId??'fixture-event',native:{runId:p.runId,sessionKey:p.sessionKey,sessionId:p.sessionId,agentId:p.agentId,historyGeneration:p.historyGeneration}},observedAt:new Date().toISOString(),...(o?{outcome:o,projectionId:a.applied.projectionId}:{})}},
 async recordNativeEvent(){},async reportModelOutcome(p,a,o,m){if(m)this.captureNativeEvent(p,m,a,o)},async afterTurn(){},async dispose(){},
};
export default {id:'nous-cognition',name:'Native completion fixture',register(api){
 api.session.state.registerSessionExtension({namespace:'nous-cognition',description:'Fixture exact native handoff'});
 api.registerContextEngine('nous-cognition',()=>engine);
 api.registerTool(()=>({name:'fixture_read',label:'Read fixture',description:'Read one synthetic saved official result',parameters:{type:'object',properties:{},additionalProperties:false},async execute(id){log('worker-tool',{toolCallId:id});return {content:[{type:'text',text:'SYNTHETIC_OFFICIAL_TITLE'}],details:{source:'fixture-only'}}}}),{names:['fixture_read']});
 api.registerTool(ctx=>{const binding=ctx.toolBindings?.heptalogos;log('factory-binding',{sessionKey:ctx.sessionKey,binding:binding??null});if(!binding)return null;return {name:'fixture_binding',label:'Actual binding',description:'Verify current completion binding',parameters:{type:'object',properties:{},additionalProperties:false},async execute(id){if(!registered.has(binding.nativeRunId)||!admitted.has(binding.nativeRunId))throw Error('Wrong completion binding');log('binding',{nativeRunId:binding.nativeRunId,toolCallId:id,sessionKey:ctx.sessionKey});return {content:[{type:'text',text:'CURRENT_COMPLETION_BOUND'}],details:{}}}}},{names:['fixture_binding']});
 api.registerGatewayMethod('fixture.releaseCompletion',async({params,respond})=>{released=true;try{const result=await retryNousSubagentCompletion(params);respond(true,result)}catch(e){respond(false,undefined,{code:'INVALID_REQUEST',message:String(e)})}},{scope:'operator.admin'});
 api.registerChannel({plugin:{id:'completion-fixture',meta:{id:'completion-fixture',label:'Completion fixture',selectionLabel:'Fixture',docsPath:'/fixture',blurb:'No QQ traffic'},capabilities:{chatTypes:['direct','group'],blockStreaming:false},config:{listAccountIds:()=>['fixture-account'],defaultAccountId:()=> 'fixture-account',resolveAccount:()=>({accountId:'fixture-account',enabled:true,configured:true}),isEnabled:()=>true,isConfigured:()=>true},outbound:{deliveryMode:'direct',textChunkLimit:4096,resolveTarget:({to})=>({ok:true,to})},message:{durableFinal:{capabilities:{text:true},automaticUnknownSendReconciliation:false},deliveryCustody:{async adopt(p){if(!admitted.has(p.runId))throw Error('Producer not Host admitted');log('custody',{runId:p.runId,intentId:p.intentId,sourceIndex:p.sourceIndex});return {reference:'fixture-behavior:'+p.runId}},async authorize(p){if(!admitted.has(p.runId))throw Error('Producer not admitted')}},send:{async text(p){log('sent',{runId:p.runId,text:p.text,sourceIndex:p.sourceIndex,partIndex:p.partIndex});return {receipt:{platformMessageIds:['fixture-platform-message']}}}}}}});
}};
