// Copied from Heptalogos platform/model-test.ts; shared actual SDK test operation.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {DBOS} from '@dbos-inc/dbos-sdk';
import {createHash} from 'node:crypto';
import {canonicalize} from 'json-canonicalize';
import type {CallContext} from '@nous-wave/contracts/common';
import type {Models} from './index.js';
import type {ModelsOptions} from './types.js';
import {ProblemError} from '@nous-wave/contracts/common';
import type {ModelTestInput} from '@nous-wave/contracts/models';
export function createModelTest(options:{product:'hept'|'nous';installationId:string;models:Models;authorize:ModelsOptions['authorize'];beforeDispatch():void}){
 const workflow=DBOS.registerWorkflow(async(ctx:CallContext,input:ModelTestInput)=>{
  const result=await options.models.text.generate(ctx,{roleId:input.roleId,inputRefs:[],resource:input.resource,promptParams:input.promptParams,options:{prompt:input.prompt,maxOutputTokens:input.maxOutputTokens}});
  return {text:result.text,usage:result.totalUsage};
 },{name:`${options.product}.models.test`});
 const digest=(value:unknown)=>createHash('sha256').update(canonicalize(value)).digest('hex');
 return async(ctx:CallContext,input:ModelTestInput)=>{
  options.beforeDispatch();await options.authorize(ctx,'models.test',input.resource?{product:options.product,installationId:options.installationId,kind:'ModelResource',id:input.resource.id,revision:input.resource.revision}:undefined);
  const workflowID=`model-test:${ctx.trace.operationId}`;
  await DBOS.startWorkflow(workflow,{workflowID,workflowAttributes:{subjectId:ctx.subjectId,operation:'models.test'}})(ctx,input);
  const prior=(await DBOS.listWorkflows({workflowIDs:[workflowID],loadInput:true,loadOutput:false}))[0]?.input;
  if(prior&&digest(prior[1])!==digest(input))throw new ProblemError({code:'models.submission_conflict',category:'conflict',status:409,messageKey:'models.submission_conflict',message:'同一模型测试提交包含不同输入'});
  return {operationId:workflowID,statusUrl:`/v1/operations/${encodeURIComponent(workflowID)}`};
 };
}
