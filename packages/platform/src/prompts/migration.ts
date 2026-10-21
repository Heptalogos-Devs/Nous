import type {ColumnType,Kysely} from 'kysely';
import type {Migration} from 'kysely/migration';
import type {CallContext} from '@nous-wave/contracts/common';
import type {PromptChangeResult,PromptSnapshot} from '@nous-wave/contracts/prompts';
import {sql} from '../storage/index.js';
import type {PromptManifest} from './types.js';
type Timestamp=ColumnType<string,string|undefined,never>;
export interface PromptDatabase {
  promptRevisions:{id:string;promptId:string;locale:string;scopeKind:string;scopeId:string;revision:string;sourceText:string;digest:string;paramsSchemaRevision:string;dependencies:unknown;origin:unknown;actor:CallContext['actor'];reason:string|null;recordedAt:Timestamp};
  promptHeads:{promptId:string;locale:string;scopeKind:string;scopeId:string;revision:string;latestRevision:string;activeRevision:string|null};
  promptSnapshots:{id:string;digest:string;manifest:PromptManifest;recordedAt:Timestamp};
  promptChangeReceipts:{operationId:string;action:string;promptId:string;locale:string;scopeKind:string;scopeId:string;requestDigest:string;result:PromptChangeResult;actorAuthorityRef:string;consumer:CallContext['consumer'];recordedAt:Timestamp};
}
export const promptsMigration:Migration={
  async up(db:Kysely<any>){
    await db.schema.createTable('promptRevisions').addColumn('id','uuid',c=>c.primaryKey()).addColumn('promptId','text',c=>c.notNull()).addColumn('locale','text',c=>c.notNull()).addColumn('scopeKind','text',c=>c.notNull()).addColumn('scopeId','text',c=>c.notNull()).addColumn('revision','bigint',c=>c.notNull()).addColumn('sourceText','text',c=>c.notNull()).addColumn('digest','text',c=>c.notNull()).addColumn('paramsSchemaRevision','text',c=>c.notNull()).addColumn('dependencies','jsonb',c=>c.notNull()).addColumn('origin','jsonb',c=>c.notNull()).addColumn('actor','jsonb',c=>c.notNull()).addColumn('reason','text').addColumn('recordedAt',sql`timestamptz(6)`,c=>c.notNull().defaultTo(sql`clock_timestamp()`)).addUniqueConstraint('promptRevisionIdentity',['promptId','locale','scopeKind','scopeId','revision']).execute();
    await db.schema.createTable('promptHeads').addColumn('promptId','text',c=>c.notNull()).addColumn('locale','text',c=>c.notNull()).addColumn('scopeKind','text',c=>c.notNull()).addColumn('scopeId','text',c=>c.notNull()).addColumn('revision','bigint',c=>c.notNull()).addColumn('latestRevision','bigint',c=>c.notNull()).addColumn('activeRevision','bigint').addPrimaryKeyConstraint('promptHeadsPk',['promptId','locale','scopeKind','scopeId']).addForeignKeyConstraint('promptLatestFk',['promptId','locale','scopeKind','scopeId','latestRevision'],'promptRevisions',['promptId','locale','scopeKind','scopeId','revision']).addForeignKeyConstraint('promptActiveFk',['promptId','locale','scopeKind','scopeId','activeRevision'],'promptRevisions',['promptId','locale','scopeKind','scopeId','revision']).execute();
    await db.schema.createTable('promptSnapshots').addColumn('id','uuid',c=>c.primaryKey()).addColumn('digest','text',c=>c.notNull().unique()).addColumn('manifest','jsonb',c=>c.notNull()).addColumn('recordedAt',sql`timestamptz(6)`,c=>c.notNull().defaultTo(sql`clock_timestamp()`)).execute();
    await db.schema.createTable('promptChangeReceipts').addColumn('operationId','text',c=>c.primaryKey()).addColumn('action','text',c=>c.notNull()).addColumn('promptId','text',c=>c.notNull()).addColumn('locale','text',c=>c.notNull()).addColumn('scopeKind','text',c=>c.notNull()).addColumn('scopeId','text',c=>c.notNull()).addColumn('requestDigest','text',c=>c.notNull()).addColumn('result','jsonb',c=>c.notNull()).addColumn('actorAuthorityRef','text',c=>c.notNull()).addColumn('consumer','jsonb',c=>c.notNull()).addColumn('recordedAt',sql`timestamptz(6)`,c=>c.notNull().defaultTo(sql`clock_timestamp()`)).execute();
  },
  async down(db:Kysely<any>){for(const table of ['promptChangeReceipts','promptSnapshots','promptHeads','promptRevisions'])await db.schema.dropTable(table).execute();},
};
