import { z } from 'zod';
import { ConsumerSchema, PolicySchema, RefSchema, TimePointSchema, TimeRangeSchema } from './common.js';
export const MaterialSourceSchema = z.object({ ref: RefSchema.optional(), inputs:z.array(RefSchema).default([]), roots: z.array(RefSchema).default([]), originVersion: z.string().optional() });
export interface SourceResolution { resolvedRefs:import('./common.js').Ref[];roots:import('./common.js').Ref[];policies:import('./common.js').Policy[] }
export const BlobMetadataSchema=z.object({digest:z.string().min(1),bytes:z.number().int().nonnegative(),mediaType:z.string().min(1)});
export type BlobMetadata=z.infer<typeof BlobMetadataSchema>;
export interface BlobResult extends BlobMetadata {ref:import('./common.js').Ref}
export const MaterialCaptureSchema = z.object({
  kind: z.string(), mediaType: z.string(), title: z.string().optional(), text: z.string().optional(),
  blobRef: RefSchema.optional(), source: MaterialSourceSchema, occurred: TimeRangeSchema.optional(), policy: PolicySchema.optional(),
});
export const MaterialViewSchema = MaterialCaptureSchema.extend({ ref: RefSchema, policy: PolicySchema, scope: ConsumerSchema, recordedAt: z.iso.datetime() });
export const MaterialPublishSchema = z.object({ ref: RefSchema, title: z.string().optional() });
export type MaterialView = z.infer<typeof MaterialViewSchema>;
export const PortionSchema = z.object({ ref: RefSchema, coverage: z.string().optional() });
export const EncounterAcceptSchema = z.object({
  consumer: ConsumerSchema, invocationId: z.string(), step: z.union([z.string(),z.number().int().nonnegative()]),
  contextViewRef: RefSchema.optional(), portions: z.array(PortionSchema), resultRef: RefSchema.optional(),
  observed: TimePointSchema, occurred: TimeRangeSchema.optional(), activityRef: RefSchema.optional(), contextRef: RefSchema.optional(),
  purpose: z.string(), completion: z.enum(['complete','usable_partial']).default('complete'),
});
export type EncounterAccept = z.infer<typeof EncounterAcceptSchema>;
export const EncounterViewSchema = EncounterAcceptSchema.extend({ ref: RefSchema, recordedAt: z.iso.datetime() });
export type EncounterView = z.infer<typeof EncounterViewSchema>;
export const ExperienceItemSchema = z.object({ ref: RefSchema, encounterRef: RefSchema, observed: TimePointSchema, portions: z.array(PortionSchema), activityRef: RefSchema.optional(), contextRef: RefSchema.optional(), sourceRoots: z.array(RefSchema) });
export type ExperienceItem = z.infer<typeof ExperienceItemSchema>;
export const ExperienceListSchema=z.object({refs:z.array(RefSchema).max(200).optional(),range:TimeRangeSchema.optional(),cursor:z.string().optional(),limit:z.number().int().positive().max(200).default(100)});
export const ExperiencePageSchema=z.object({items:z.array(ExperienceItemSchema),nextCursor:z.string().optional()});
export type ExperiencePage=z.infer<typeof ExperiencePageSchema>;

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({MaterialSourceSchema,MaterialCaptureSchema,MaterialViewSchema,MaterialPublishSchema,PortionSchema,EncounterAcceptSchema,EncounterViewSchema,ExperienceItemSchema,ExperienceListSchema,ExperiencePageSchema})) z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });
