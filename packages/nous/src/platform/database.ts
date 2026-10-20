import { Migrator } from 'kysely/migration';
import type { Kysely } from 'kysely';
import { createRequire } from 'node:module';
const { Kysely:CjsKysely, PostgresDialect, CamelCasePlugin } = createRequire(import.meta.url)('kysely') as typeof import('kysely');
import type { Generated,ColumnType } from 'kysely';
import { Pool } from 'pg';
import { KyselyDataSource } from '@dbos-inc/kysely-datasource';
import type { KnowledgeBody } from '@nous-wave/contracts/knowledge';
import type { Policy, Ref, TimePoint, TimeRange } from '@nous-wave/contracts/common';
import type { EncounterAccept, ExperienceItem, MaterialView } from '@nous-wave/contracts/materials';
import * as initial from '../../../../migrations/001-initial.js';
import * as organization from '../../../../migrations/002-cognition-organization.js';
import * as materialBlobs from '../../../../migrations/004-material-blobs.js';
import * as navigation from '../../../../migrations/005-knowledge-navigation.js';
import * as scope from '../../../../migrations/006-consumer-scope.js';
import * as settings from '../../../../migrations/007-settings-history.js';
import * as modelInvocations from '../../../../migrations/003-model-invocations.js';

export interface Database {
  subjects: {id:string;displayName:string;capabilities:string[];status:string;changeCursor:Generated<string>;createdAt:string;branchedFrom:Ref|null};
  subjectBindings: {id:string;subjectId:string;installationId:string;remoteSubjectId:string;connectionRef:string;recordedAt:string};
  readableRefs: {subjectId:string;alias:string;objectId:string;kind:string};
  materials: {id:string;subjectId:string;currentVersion:number;policy:Policy;status:string;createdAt:string};
  materialVersions: {id:string;materialId:string;version:number;body:MaterialView;recordedAt:string};
  blobs: {id:string;subjectId:string;submissionKey:string;digest:string;bytes:ColumnType<string,number,number>;mediaType:string;recordedAt:string};
  encounters: {id:string;subjectId:string;stepKey:string;inputHash:string;body:EncounterAccept;recordedAt:string};
  experienceItems: {id:string;subjectId:string;encounterId:string;body:ExperienceItem;observedAt:string;clockId:string;recordedAt:string;processed:boolean};
  knowledgeObjects: {id:string;subjectId:string;kind:string;currentRevision:number;status:string;policy:Policy;alias:string;createdAt:string};
  knowledgeRevisions: {id:string;objectId:string;revision:number;body:KnowledgeBody;sourceRefs:Ref[];sourceRoots:Ref[];formed:TimePoint;valid:TimeRange|null;recordedAt:string;formation:string;stance:string;confidence:number;actor:unknown;reason:string|null};
  knowledgeConcepts:{subjectId:string;objectId:string;revision:number;conceptId:string};
  sourceLinks: {id:string;subjectId:string;fromRef:Ref;toRef:Ref;relation:string;roots:Ref[]};
  objectRelations: {id:string;relationId:string;revision:number;subjectId:string;fromId:string;toId:string;relation:string;weight:number;evidenceRefs:Ref[];roots:Ref[];recordedAt:string;formed:TimePoint;valid:TimeRange|null};
  workContexts: {id:string;subjectId:string;revision:number;body:unknown;recordedAt:string};
  contextViews: {id:string;subjectId:string;body:unknown;recordedAt:string};
  formationArtifacts: {id:string;subjectId:string;body:unknown;recordedAt:string};
  useEvents: {id:string;subjectId:string;stepKey:string;kind:string;refs:Ref[];basisRefs:Generated<Ref[]>;recordedAt:string};
  resources: {id:string;subjectId:string;revision:number;body:unknown;recordedAt:string};
  episodeMembers: {episodeId:string;revision:number;experienceId:string;subjectId:string;track:string};
  motivationEvaluations: {id:string;subjectId:string;objectId:string;criterionRevision:number;body:unknown;recordedAt:string};
  resourceVersions: {id:string;resourceId:string;version:string;materialRef:Ref;recordedAt:string};
  subjectChanges: {subjectId:string;cursor:string;changedRefs:Ref[];kinds:string[];recordedAt:string};
  settingsOverrides: {scope:string;scopeId:string;revision:number;body:unknown;recordedAt:string};
  settingsHistory: {id:string;scope:string;scopeId:string;revision:number;body:unknown;previousBody:unknown;reason:string|null;actor:unknown;recordedAt:string};
  modelResources: {id:string;body:unknown;recordedAt:string};
  modelInvocations: {id:string;subjectId:string;role:string;modelResourceId:string;status:string;reservedCost:number;actualCost:number|null;usage:unknown;providerRequestId:string|null;inputRefs:Ref[];workflowId:string;traceId:string;startedAt:string;completedAt:string|null;request:Generated<unknown>;requestHash:Generated<string>;completionHash:Generated<string|null>};
}
export async function openDatabase(databaseUrl: string) {
  const pool = new Pool({connectionString:databaseUrl,max:10});
  const db = new CjsKysely<Database>({dialect:new PostgresDialect({pool}),plugins:[new CamelCasePlugin({maintainNestedObjectKeys:true})]});
  const migrator = new Migrator({db,provider:{getMigrations:async()=>({'001-initial':initial,'002-cognition-organization':organization,'003-model-invocations':modelInvocations,'004-material-blobs':materialBlobs,'005-knowledge-navigation':navigation,'006-consumer-scope':scope,'007-settings-history':settings})}});
  const result = await migrator.migrateToLatest();
  if (result.error) { await db.destroy(); throw result.error; }
  const datasource = new KyselyDataSource<Database>('nous',db);
  return {db,pool,datasource};
}
