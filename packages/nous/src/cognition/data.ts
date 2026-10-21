import { z } from 'zod';
import { InstantInputSchema as InstantSchema, RefSchema,ConsumerSchema,type CallContext,type Ref } from '@nous-wave/contracts/common';
import { ContextViewSchema, FormationArtifactSchema, WorkContextBodySchema } from '@nous-wave/contracts/cognition';
import type { Storage } from '../platform/context.js';
import { createRecordDataOwner, subjectScope } from '@nous-wave/platform/data';
export function createCognitionData(store:Storage,options:{resolve(ctx:CallContext,refs:Ref[]):Promise<unknown>}){
 const body=(schema:z.ZodType)=>(row:Record<string,unknown>,id:string)=>z.strictObject({id:z.uuid(),subject_id:z.literal(id),body:schema,recorded_at:InstantSchema}).parse(row);
 return createRecordDataOwner(Object.assign(Object.create(store),{product:'nous' as const}),{name:'cognition',collections:[
  {table:'work_contexts',scope:subjectScope,validate(row,id){body(WorkContextBodySchema.extend({scope:ConsumerSchema}))(row,id);z.number().int().positive().parse(row.revision);}},
  {table:'context_views',scope:subjectScope,validate:body(ContextViewSchema)},
  {table:'formation_artifacts',scope:subjectScope,validate:body(FormationArtifactSchema)},
  {table:'use_events',scope:subjectScope,validate(row,id){z.strictObject({id:z.uuid(),subject_id:z.literal(id),step_key:z.string(),kind:z.string(),refs:z.array(RefSchema),basis_refs:z.array(RefSchema),recorded_at:InstantSchema}).parse(row);}},
 ],async beforeRead(ctx,row){
  const value=row.record.body;
  if(row.collection==='context_views'){const view=ContextViewSchema.parse(value);await options.resolve(ctx,view.inputPortions.map(portion=>({...portion.ref,selector:portion.selector??portion.ref.selector})));}
  if(row.collection==='work_contexts'){const work=WorkContextBodySchema.parse(value);await options.resolve(ctx,[...work.currentRefs,...work.relatedRefs]);}
  if(row.collection==='formation_artifacts'){const artifact=FormationArtifactSchema.parse(value);await options.resolve(ctx,[...artifact.sourceRefs,...artifact.experienceRefs,...(artifact.modelOutputRef?[artifact.modelOutputRef]:[])]);}
 }});
}
