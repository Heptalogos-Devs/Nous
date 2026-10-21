import { z } from 'zod';
import { ConsumerSchema, InstantSchema, PolicySchema, ProblemSchema, RefSchema, SubjectIdSchema, TimePointSchema } from './common.js';
const count=z.string().regex(/^\d+$/), digest=z.string().min(1);
export const OwnerCutSchema=z.strictObject({owner:z.string(),schemaVersion:z.number().int().positive(),subjectId:SubjectIdSchema,realRecordedCut:InstantSchema,selectionRef:RefSchema,count});
export const DataComponentSchema=z.strictObject({owner:z.string(),schemaVersion:z.number().int().positive(),path:z.string(),digest,bytes:count});
export const BlobExportEntrySchema=z.strictObject({blobRef:RefSchema,digest,bytes:count,mediaType:z.string(),path:z.string()});
export const PendingResourceSchema=z.strictObject({owner:z.string(),ref:RefSchema.optional(),reasonCode:z.string(),operationId:z.string().optional()});
export const BindingRequirementSchema=z.strictObject({id:z.string(),kind:z.enum(['secret','provider','channel_account','time_authority','native_runtime','resource']),logicalName:z.string(),sourceRef:RefSchema.optional(),capabilities:z.array(z.string())});
export const SubjectBundleManifestSchema=z.strictObject({format:z.literal('nous-hept-subject'),version:z.literal(1),bundleId:z.uuidv7(),mode:z.literal('restore'),subject:z.strictObject({id:SubjectIdSchema,name:z.string()}),sources:z.array(z.strictObject({product:z.enum(['nous','hept']),installationId:z.string(),productVersion:z.string()})),createdAt:TimePointSchema,cuts:z.array(OwnerCutSchema),components:z.array(DataComponentSchema),blobs:z.array(BlobExportEntrySchema),requiredBindings:z.array(BindingRequirementSchema),pending:z.array(PendingResourceSchema)});
export const DataArtifactSchema=z.strictObject({ref:RefSchema,blobRef:RefSchema,mediaType:z.string(),digest,bytes:count,createdAt:TimePointSchema,expiresAt:TimePointSchema.optional(),policy:PolicySchema,consumer:ConsumerSchema.optional(),sourceRefs:z.array(RefSchema)});
export const DataExportSchema=z.strictObject({subjectId:SubjectIdSchema,mode:z.literal('restore').default('restore')});
export const DataArtifactReadSchema=z.strictObject({artifactRef:RefSchema});
export const ImportArtifactResultSchema=z.strictObject({artifactRef:RefSchema,digest,bytes:count});
export const ExpectedDataTargetSchema=z.strictObject({ref:RefSchema,expectedRevision:z.number().int().nonnegative().optional(),expectedCursor:count.optional()});
export const ImportPlanSchema=z.strictObject({id:z.uuidv7(),artifactRef:RefSchema,manifestDigest:digest,targetMode:z.literal('restore'),subjectId:SubjectIdSchema,ownerValidations:z.array(z.strictObject({owner:z.string(),schemaVersion:z.number().int().positive(),compatible:z.boolean(),problems:z.array(ProblemSchema)})),expectedTargets:z.array(ExpectedDataTargetSchema),requiredBindings:z.array(BindingRequirementSchema),pending:z.array(PendingResourceSchema)});
export const DataImportSchema=z.strictObject({planId:z.uuidv7(),expectedTargets:z.array(ExpectedDataTargetSchema)});
export const DataImportResultSchema=z.strictObject({subjectId:SubjectIdSchema,status:z.literal('paused'),pending:z.array(PendingResourceSchema)});
export const DataPurgeSelectionSchema=z.strictObject({subjectId:SubjectIdSchema});
export const PurgePlanSchema=z.strictObject({id:z.uuidv7(),selectionRef:RefSchema,subjectId:SubjectIdSchema,expectedTargets:z.array(ExpectedDataTargetSchema)});
export const PurgeReceiptSchema=z.strictObject({id:z.uuidv7(),selectionRef:RefSchema,recordedAt:TimePointSchema,cleared:z.array(z.strictObject({owner:z.string(),count,bytes:count.optional()})),pending:z.array(PendingResourceSchema)});
export const DataReceiptReadSchema=z.strictObject({operationId:z.string().min(1)});
export const DataReceiptSchema=z.discriminatedUnion('operationName',[
 z.strictObject({operationId:z.string(),operationName:z.literal('data.export'),recordedAt:TimePointSchema,result:DataArtifactSchema}),
 z.strictObject({operationId:z.string(),operationName:z.literal('data.import'),recordedAt:TimePointSchema,result:DataImportResultSchema}),
 z.strictObject({operationId:z.string(),operationName:z.literal('data.purge'),recordedAt:TimePointSchema,result:PurgeReceiptSchema}),
]);
export const DataSettingsSchema=z.strictObject({maxBundleBytes:z.number().int().positive().safe(),maxExpandedBytes:z.number().int().positive().safe(),maxEntries:z.number().int().positive().safe(),temporaryRetentionHours:z.number().int().positive().max(720)});
export type DataReceipt=z.infer<typeof DataReceiptSchema>;
export type OwnerCut=z.infer<typeof OwnerCutSchema>;
export type SubjectBundleManifest=z.infer<typeof SubjectBundleManifestSchema>;
export type DataArtifact=z.infer<typeof DataArtifactSchema>;
export type ImportPlan=z.infer<typeof ImportPlanSchema>;
export type PurgePlan=z.infer<typeof PurgePlanSchema>;
export type PurgeReceipt=z.infer<typeof PurgeReceiptSchema>;
export type PendingResource=z.infer<typeof PendingResourceSchema>;
for(const [id,schema]of Object.entries({OwnerCutSchema,DataComponentSchema,BlobExportEntrySchema,PendingResourceSchema,BindingRequirementSchema,SubjectBundleManifestSchema,DataArtifactSchema,DataExportSchema,DataArtifactReadSchema,ImportArtifactResultSchema,ExpectedDataTargetSchema,ImportPlanSchema,DataImportSchema,DataImportResultSchema,DataPurgeSelectionSchema,PurgePlanSchema,PurgeReceiptSchema,DataReceiptReadSchema,DataReceiptSchema,DataSettingsSchema}))z.globalRegistry.add(schema,{id:id.replace(/Schema$/,'')});
