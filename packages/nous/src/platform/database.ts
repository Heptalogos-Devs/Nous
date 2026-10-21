import {modelArtifactsMigration,type ModelArtifactsDatabase} from '../models/migration.js';
import type { Generated,ColumnType } from 'kysely';
import { createStorage } from '@nous-wave/platform/storage';
import { delegationMigration, secretMigration, type DelegationDatabase, type SecretDatabase } from '@nous-wave/platform/credentials';
import { settingsMigration,settingsOriginMigration, type SettingsDatabase } from '@nous-wave/platform/settings';
import {timeMigration,type TimeDatabase} from '../time/migration.js';
import type { KnowledgeBody } from '@nous-wave/contracts/knowledge';
import type { Policy, Ref, TimePoint, TimeRange } from '@nous-wave/contracts/common';
import type { EncounterAccept, ExperienceItem, MaterialView } from '@nous-wave/contracts/materials';
import * as initial from '../../migrations/001-initial.js';
import * as organization from '../../migrations/002-cognition-organization.js';
import * as materialBlobs from '../../migrations/004-material-blobs.js';
import * as navigation from '../../migrations/005-knowledge-navigation.js';
import * as scope from '../../migrations/006-consumer-scope.js';
import {modelMigration,type ModelsDatabase} from '@nous-wave/platform/models';
import {promptsMigration,type PromptDatabase} from '@nous-wave/platform/prompts';

export interface Database extends DelegationDatabase, SecretDatabase, SettingsDatabase, TimeDatabase, ModelsDatabase, PromptDatabase, ModelArtifactsDatabase {
  subjects: {id:string;displayName:string;capabilities:string[];status:string;changeCursor:Generated<string>;createdAt:string;branchedFrom:Ref|null};
  subjectBindings: {id:string;subjectId:string;installationId:string;remoteSubjectId:string;connectionRef:string;recordedAt:string};
  readableRefs: {subjectId:string;alias:string;objectId:string;kind:string};
  materials: {id:string;subjectId:string;currentVersion:number;policy:Policy;status:string;createdAt:string};
  materialVersions: {id:string;materialId:string;version:number;body:MaterialView;recordedAt:string};
  blobs: {id:string;subjectId:string;submissionKey:string;digest:string;bytes:ColumnType<string,number,number>;mediaType:string;recordedAt:string};
  encounters: {id:string;subjectId:string;stepKey:string;inputHash:string;body:EncounterAccept;recordedAt:string};
  experienceItems: {id:string;subjectId:string;encounterId:string;body:ExperienceItem;observedAt:string;domainId:string;recordedAt:string;processed:boolean};
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
}
export async function openDatabase(databaseUrl: string, maxConnections:number) {
  const storage = createStorage<Database>({
    pool:{connectionString:databaseUrl,max:maxConnections,application_name:'nous-wave'},
    datasourceName:'nous',
  });
  const result = await storage.migrate({getMigrations:async()=>({
    '001-initial':initial,'002-cognition-organization':organization,
    '004-material-blobs':materialBlobs,
    '005-knowledge-navigation':navigation,'006-consumer-scope':scope,
    '008-settings':settingsMigration,'009-delegations':delegationMigration,'010-secrets':secretMigration,'011-settings-origin':settingsOriginMigration,'012-time':timeMigration,'013-models':modelMigration,'014-prompts':promptsMigration,'015-model-artifacts':modelArtifactsMigration,
  })});
  if(result.error) { await storage.close(); throw result.error; }
  return storage;
}
