import { z } from 'zod';
import { sql } from 'kysely';
import { InstantInputSchema as InstantSchema, RefSchema } from '@nous-wave/contracts/common';
import { ResourceBodySchema } from '@nous-wave/contracts/cognition';
import type { Storage } from '../platform/context.js';
import { createRecordDataOwner, subjectScope } from '@nous-wave/platform/data';
import {mayRead} from '../platform/security.js';
import {problem} from '@nous-wave/platform/errors';
export function createResourceData(store:Storage){const owner=createRecordDataOwner(Object.assign(Object.create(store),{product:'nous' as const}),{name:'resources',collections:[
 {table:'resources',scope:subjectScope,validate(row,id){z.strictObject({id:z.uuid(),subject_id:z.literal(id),revision:z.number().int().positive(),body:ResourceBodySchema,recorded_at:InstantSchema}).parse(row);}},
 {table:'resource_versions',scope:id=>sql<boolean>`t.resource_id in(select id from resources where subject_id=${id}::uuid)`,validate(row){z.strictObject({id:z.uuid(),resource_id:z.uuid(),version:z.string(),material_ref:RefSchema,recorded_at:InstantSchema}).parse(row);}},
],async beforeRead(ctx,row){if(row.collection==='resources'&&!mayRead(ctx,ResourceBodySchema.parse(row.record.body).policy))throw problem('data.resource_policy','forbidden','资料源不允许本次导出用途或听众');}});return {...owner,async prepareExport(ctx:Parameters<typeof owner.prepareExport>[0],id:string,db:Storage['db']){const result=await owner.prepareExport(ctx,id,db);await sql`update resources_export_members set record=jsonb_set(record,'{body}',(record->'body'-'root')||jsonb_build_object('root','','bindingRequired',true)) where selection_id=${id}::uuid and collection='resources' and record->'body'->>'kind'='directory'`.execute(db);return result;},async bindings(ctx:Parameters<typeof owner.readSelection>[0],id:string){const requirements=[];for await(const row of owner.readSelection(ctx,id)){if(row.collection==='resources'&&ResourceBodySchema.parse(row.record.body).kind==='directory')requirements.push({id:`resource:${row.record.id}`,kind:'resource' as const,logicalName:ResourceBodySchema.parse(row.record.body).title,sourceRef:{product:'nous' as const,installationId:store.installationId,subjectId:ctx.subjectId,kind:'Resource',id:String(row.record.id)},capabilities:['directory.read']});}return requirements;}};}
