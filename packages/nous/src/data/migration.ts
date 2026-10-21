import { sql,type Kysely,type Generated } from 'kysely';
import type { Consumer,Policy,Ref } from '@nous-wave/contracts/common';
import type { ImportPlan,OwnerCut,PurgePlan } from '@nous-wave/contracts/data';
export interface DataDatabase {
 dataSelections:{id:string;subjectId:string;recordedAt:string;cuts:OwnerCut[];settings:unknown;kind:'export'|'purge';consumer:Consumer;purpose:string;audience:string[]};
 dataArtifacts:{id:string;kind:'DataArtifact'|'ImportArtifact';subjectId:string|null;blobRef:Ref;digest:string;bytes:string;mediaType:string;policy:Policy;consumer:Consumer;sourceRefs:Ref[];selectionId:string|null;createdAt:string;expiresAt:string|null;status:'available'|'closed'};
 dataManifests:{artifactId:string;body:unknown};
 dataImportPlans:{id:string;artifactId:string;body:ImportPlan};
 dataPurgePlans:{id:string;body:PurgePlan};
 dataReceipts:{operationId:string;operationName:string;requestDigest:string;actorScopeDigest:string;subjectId:string|null;result:unknown;cleanup:unknown|null;recordedAt:string};
}
export const dataMigration={async up(db:Kysely<unknown>){
 for(const owner of ['identity','materials','knowledge','cognition','resources'])await sql`create table ${sql.table(`${owner}_export_members`)}(selection_id uuid not null,ordinal bigint generated always as identity,collection text not null,record jsonb not null,primary key(selection_id,ordinal))`.execute(db);
 await sql`create table data_selections(id uuid primary key,subject_id uuid not null,recorded_at timestamptz(6) not null,cuts jsonb not null,settings jsonb not null,kind text not null,consumer jsonb not null,purpose text not null,audience jsonb not null);
 create table data_artifacts(id uuid primary key,kind text not null,subject_id uuid,blob_ref jsonb not null,digest text not null,bytes bigint not null check(bytes>=0),media_type text not null,policy jsonb not null,consumer jsonb not null,source_refs jsonb not null,selection_id uuid,created_at timestamptz(6) not null,expires_at timestamptz(6),status text not null);
 create table data_manifests(artifact_id uuid primary key references data_artifacts(id) on delete cascade,body jsonb not null);
 create table data_import_plans(id uuid primary key,artifact_id uuid not null references data_artifacts(id),body jsonb not null);
 create table data_purge_plans(id uuid primary key,body jsonb not null);
 create table data_receipts(operation_id text primary key,operation_name text not null,request_digest text not null,actor_scope_digest text not null,subject_id uuid,result jsonb not null,cleanup jsonb,recorded_at timestamptz(6) not null);
 alter table blobs alter column subject_id drop not null;
 drop index blobs_submission;create unique index blobs_submission on blobs(subject_id,submission_key) nulls not distinct;
 create index data_artifact_subject on data_artifacts(subject_id,status);
 create index data_selection_subject on data_selections(subject_id)`.execute(db);
 },async down(){throw new Error('Use a fresh development database.');}};
