// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import {sql,type Kysely,type ColumnType} from 'kysely';
import type {CallContext,Consumer,Ref,Problem} from '@nous-wave/contracts/common';
import type {InvocationSnapshot,ModelInvocationIdentity,NormalizedUsage,ModelKind,TokenEstimate,ModelPreparation} from '@nous-wave/contracts/models';
import type {TimeCaptureReceipt} from '@nous-wave/contracts/time';
import type {BudgetConstraint} from './types.js';
type JsonColumn<T>=ColumnType<T,T|string,T|string>;
export interface ModelInvocationRow {
 id:string;logicalOperationId:string;physicalOrdinal:number;attemptNo:number;workflowId:string|null;stepNo:number|null;batchNo:number|null;
 subjectId:string|null;consumer:JsonColumn<Consumer>;purpose:string;audience:JsonColumn<string[]>;authorityRef:string;traceId:string;roleId:string;modelKind:ModelKind;
 status:'prepared'|'dispatched'|'completed'|'not_sent'|'unknown';requestDigest:string;completionDigest:string|null;
 configSnapshot:JsonColumn<InvocationSnapshot>;resourceId:string;resourceRevision:number;actualProvider:string;actualModelId:string;
 preparation:JsonColumn<ModelPreparation|null>;
 inputSources:JsonColumn<Ref[]>;semanticInputRefs:JsonColumn<Ref[]>;outputRefs:JsonColumn<Ref[]>;
 observedCapture:JsonColumn<TimeCaptureReceipt|null>;formedCapture:JsonColumn<TimeCaptureReceipt|null>;
 tokenEstimate:JsonColumn<TokenEstimate>;maxOutputTokens:number;usage:JsonColumn<NormalizedUsage|null>;rawUsage:JsonColumn<unknown>;
 budgetSnapshot:JsonColumn<{constraints:BudgetConstraint[]}>;budgetKeys:JsonColumn<string[]>;
 reservedCost:string|null;actualCost:string|null;currency:string|null;actualTokens:string|null;
 providerRequestId:string|null;problem:JsonColumn<Problem|null>;recordedAt:string;completedAt:string|null;
 nativeIdentity:JsonColumn<unknown>;adoptedAt:string|null;
}
export interface ModelsDatabase {modelInvocations:ModelInvocationRow}
export const modelMigration={
 async up(db:Kysely<unknown>){
  await sql`create table model_invocations(
   id uuid primary key,logical_operation_id text not null,physical_ordinal integer not null check(physical_ordinal>=0),attempt_no integer not null check(attempt_no>0),workflow_id text,step_no integer,batch_no integer,
   subject_id uuid,consumer jsonb not null,purpose text not null,audience jsonb not null,authority_ref text not null,trace_id text not null,role_id text not null,model_kind text not null check(model_kind in('language','embedding','reranking','transcription','speech')),
   status text not null check(status in('prepared','dispatched','completed','not_sent','unknown')),request_digest text not null,completion_digest text,
   config_snapshot jsonb not null,preparation jsonb,resource_id text not null,resource_revision integer not null,actual_provider text not null,actual_model_id text not null,
   input_sources jsonb not null,semantic_input_refs jsonb not null,output_refs jsonb not null,observed_capture jsonb,formed_capture jsonb,
   token_estimate jsonb not null,max_output_tokens integer not null,usage jsonb,raw_usage jsonb,budget_snapshot jsonb not null,budget_keys jsonb not null,
   reserved_cost numeric,actual_cost numeric,currency text,actual_tokens bigint,provider_request_id text,problem jsonb,
   recorded_at timestamptz(6) not null,completed_at timestamptz(6),native_identity jsonb,adopted_at timestamptz(6),
   unique(logical_operation_id,physical_ordinal,attempt_no),check(reserved_cost is null or reserved_cost>=0),check(actual_cost is null or actual_cost>=0)
  );create index model_invocations_subject_page on model_invocations(subject_id,recorded_at desc,id desc);
  create index model_invocations_resource_active on model_invocations(resource_id,status);
  create index model_invocations_budget_keys on model_invocations using gin(budget_keys)`.execute(db);
 },
 async down(db:Kysely<unknown>){await sql`drop table model_invocations`.execute(db);},
};
