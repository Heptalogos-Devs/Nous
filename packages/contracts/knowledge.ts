import { z } from 'zod';
import { PolicySchema, RefSchema, TimePointSchema, TimeRangeSchema, TimePointInputSchema, TimeRangeInputSchema, InstantInputSchema, InstantSchema } from './common.js';

export const KnowledgeKindSchema = z.enum(['Memory','Episode','Journal','Schema','Concept','Self','Social','LanguageConvention','Motivation']);
export type KnowledgeKind = z.infer<typeof KnowledgeKindSchema>;
const Content = z.object({ content: z.string().min(1), title: z.string().optional(), entityRefs: z.array(RefSchema).default([]), conceptRefs: z.array(RefSchema).default([]), tags: z.array(z.string()).default([]) });
export const MemorySchema = Content.extend({ kind: z.literal('Memory'), role: z.enum(['experience','fact','reported','preference','procedure','interpretation']).default('fact') });
export const EpisodeSchema = Content.extend({ kind: z.literal('Episode'), track: z.enum(['life','social','work']), range: TimeRangeSchema, members: z.array(RefSchema), participants: z.array(RefSchema).default([]), placeRef: RefSchema.optional(), parentRef: RefSchema.optional() });
export const JournalSchema = Content.extend({ kind: z.literal('Journal'), range: TimeRangeSchema, experienceRefs: z.array(RefSchema), perspectiveAt: TimePointSchema, mood:z.string().optional(),outlook:z.string().optional() });
export const CognitiveSchemaSchema = Content.extend({ kind: z.literal('Schema'), context: z.string(), supports: z.array(RefSchema), exceptions: z.array(RefSchema), openQuestions: z.array(z.string()).default([]), prediction: z.string().optional() });
export const ConceptSchema = Content.extend({ kind: z.literal('Concept'), name: z.string().min(1), aliases: z.array(z.string()).default([]), language: z.string().default('zh') });
export const SelfSchema = Content.extend({ kind: z.literal('Self'), aspect: z.enum(['identity','role','preference','value','expression','skill','domain_knowledge','limitation','narrative']), context: z.string().optional(), stability: z.enum(['developing','stable']).default('developing') });
export const SocialSchema = Content.extend({ kind: z.literal('Social'), aspect: z.enum(['person','relation','group']), fromRef: RefSchema, toRef: RefSchema.optional(), dimension: z.string().optional(), degree: z.number().min(-1).max(1).optional(), domain: z.string().optional(), context: z.string().optional() });
export const LanguageConventionSchema = Content.extend({ kind: z.literal('LanguageConvention'), form: z.string(), meaning: z.string(), usage: z.string(), scopeRefs: z.array(RefSchema), examples: z.array(z.string()) });
export const FactPredicateSchema=z.object({path:z.array(z.string()),operator:z.enum(['eq','neq','gt','gte','lt','lte','contains','exists']),value:z.union([z.string(),z.number(),z.boolean(),z.null()]).optional()});
export const CriterionSchema = z.object({ mode: z.enum(['achieve','terminate','maintain','avoid']), range: TimeRangeSchema, condition: z.string(), predicate:FactPredicateSchema.optional(), requiredCoverage: z.number().min(0).max(1).default(1) });
export const MotivationSchema = Content.extend({ kind: z.literal('Motivation'), aspect: z.enum(['desire','concern','evaluable_expectation']), importance: z.number().min(0).max(1), criterion: CriterionSchema.optional() });
export const KnowledgeBodySchema = z.discriminatedUnion('kind', [MemorySchema,EpisodeSchema,JournalSchema,CognitiveSchemaSchema,ConceptSchema,SelfSchema,SocialSchema,LanguageConventionSchema,MotivationSchema]);
export type KnowledgeBody = z.infer<typeof KnowledgeBodySchema>;
export const KnowledgeBodyInputSchema=z.discriminatedUnion('kind',[MemorySchema,EpisodeSchema.extend({range:TimeRangeInputSchema}),JournalSchema.extend({range:TimeRangeInputSchema,perspectiveAt:TimePointInputSchema}),CognitiveSchemaSchema,ConceptSchema,SelfSchema,SocialSchema,LanguageConventionSchema,MotivationSchema.extend({criterion:CriterionSchema.extend({range:TimeRangeInputSchema}).optional()})]);
export const KnowledgeCreateSchema = z.object({
  kind: KnowledgeKindSchema, body: KnowledgeBodyInputSchema, sourceRefs: z.array(RefSchema), formed: TimePointInputSchema,
  valid: TimeRangeInputSchema.optional(), formation: z.enum(['observed','told','read','inferred','reflection','initial_setting','operator_edit']),
  stance: z.enum(['accepted','uncertain','hypothesis','disputed']), confidence: z.number().min(0).max(1).default(0.5), policy: PolicySchema.optional(),
});
export type KnowledgeCreate = z.infer<typeof KnowledgeCreateSchema>;
export const KnowledgeReviseSchema = KnowledgeCreateSchema.extend({ id: z.string(), expectedRevision: z.number().int().positive(), reason: z.string().min(1) });
export const KnowledgeViewSchema = KnowledgeCreateSchema.extend({
  body:KnowledgeBodySchema,formed:TimePointSchema,valid:TimeRangeSchema.optional(),ref: RefSchema, alias: z.string(), revision: z.number().int().positive(), sourceRoots: z.array(RefSchema),
  recorded: TimePointSchema, policy: PolicySchema, status: z.enum(['active','suppressed','retired','purged']),
});
export type KnowledgeView = z.infer<typeof KnowledgeViewSchema>;
export const KnowledgeReadSchema = z.object({ id: z.string(), revision: z.number().int().positive().optional(), knowledgeAt: TimePointInputSchema.optional(), recordedAsOf: InstantInputSchema.optional() });
export const KnowledgeListSchema = z.object({ kinds: z.array(KnowledgeKindSchema).optional(), knowledgeAt: TimePointInputSchema.optional(), recordedAsOf: InstantInputSchema.optional(), includeInactive: z.boolean().optional(),includeHistory:z.boolean().optional(),range:TimeRangeInputSchema.optional(), ids:z.array(z.uuid()).max(200).optional(), entities:z.array(z.string()).optional(),entityMatch:z.enum(['any','all']).optional(),tags:z.array(z.string()).optional(),tagMatch:z.enum(['any','all']).optional(),refs:z.array(z.string()).optional(),sources:z.array(z.string()).optional(),conceptIds:z.array(z.string()).optional(), cursor:z.string().optional(), limit: z.number().int().positive().max(200).default(100) });
export const KnowledgePageSchema=z.object({items:z.array(KnowledgeViewSchema),nextCursor:z.string().optional()});
export type KnowledgePage=z.infer<typeof KnowledgePageSchema>;
export const KnowledgeHistorySchema=z.object({cursor:z.string().optional(),limit:z.number().int().positive().max(200).default(100),knowledgeAt:TimePointInputSchema.optional(),recordedAsOf:InstantInputSchema.optional()});
export const RelationWriteSchema=z.object({fromId:z.string(),toId:z.string(),relation:z.string().min(1),weight:z.number().default(1),evidenceRefs:z.array(RefSchema),formed:TimePointInputSchema,valid:TimeRangeInputSchema.optional()});
export const ObjectRelationSchema=RelationWriteSchema.extend({formed:TimePointSchema,id:z.uuid(),relationId:z.uuid(),revision:z.number().int().positive(),subjectId:z.uuid(),roots:z.array(RefSchema),recordedAt:InstantSchema,valid:TimeRangeSchema.nullish()});
export const RelationPageSchema=z.object({items:z.array(ObjectRelationSchema),nextCursor:z.string().optional()});
export const RelationReadSchema=z.object({knowledgeAt:TimePointInputSchema.optional(),recordedAsOf:InstantInputSchema.optional(),limit:z.number().int().positive().max(200).default(60),cursor:z.string().optional()});

const RevisionTargetSchema=z.object({id:z.string(),expectedRevision:z.number().int().positive()});
export const EpisodeOrganizeSchema=z.object({
  track:z.enum(['life','social','work']),experienceRefs:z.array(RefSchema),formed:TimePointInputSchema,reason:z.string().min(1),
  episodes:z.array(z.object({target:RevisionTargetSchema.optional(),title:z.string(),summary:z.string(),range:TimeRangeInputSchema,memberRefs:z.array(RefSchema),participantRefs:z.array(RefSchema).default([]),placeRef:RefSchema.optional(),parentRef:RefSchema.optional()})),
  replaced:z.array(RevisionTargetSchema).default([]),
});
export const JournalWriteSchema=z.object({target:RevisionTargetSchema.optional(),range:TimeRangeInputSchema,experienceRefs:z.array(RefSchema),knowledgeAt:TimePointInputSchema,formed:TimePointInputSchema,content:z.string().min(1).optional(),title:z.string().optional(),mood:z.string().optional(),outlook:z.string().optional(),reason:z.string().default('组织实际经历的日记')});
export const ConceptMergeSchema=z.object({kept:RevisionTargetSchema,merged:z.array(RevisionTargetSchema).min(1),content:z.string().optional(),formed:TimePointInputSchema,reason:z.string().min(1)});
export const ConceptSplitSchema=z.object({source:RevisionTargetSchema,branches:z.array(z.object({name:z.string(),aliases:z.array(z.string()).default([]),definition:z.string(),memberRefs:z.array(RefSchema)})).min(2),remainingDefinition:z.string().optional(),formed:TimePointInputSchema,reason:z.string().min(1)});
export const ObservationCoverageSchema=z.object({range:TimeRangeInputSchema,ratio:z.number().min(0).max(1),sourceRefs:z.array(RefSchema),missing:z.array(TimeRangeInputSchema).default([])});
export const MotivationEvaluateSchema=z.object({id:z.string(),revision:z.number().int().positive().optional(),observationRefs:z.array(RefSchema),coverage:ObservationCoverageSchema,evaluatedAt:TimePointInputSchema});
export const MotivationEvaluationSchema=z.object({id:z.string(),ref:RefSchema,criterionRef:RefSchema,criterionRevision:z.number().int().positive(),verdict:z.enum(['satisfied','violated','unknown']),applicable:z.boolean(),observationCoverage:ObservationCoverageSchema,basisRefs:z.array(RefSchema),evaluatedAt:TimePointSchema,recorded:TimePointSchema});

// Named public schemas are shared by OpenAPI references.
for (const [id, schema] of Object.entries({KnowledgeKindSchema,MemorySchema,EpisodeSchema,JournalSchema,CognitiveSchemaSchema,ConceptSchema,SelfSchema,SocialSchema,LanguageConventionSchema,CriterionSchema,MotivationSchema,KnowledgeBodySchema,KnowledgeCreateSchema,KnowledgeReviseSchema,KnowledgeViewSchema,KnowledgeReadSchema,KnowledgeListSchema,KnowledgePageSchema,KnowledgeHistorySchema,RelationReadSchema,RelationWriteSchema,ObjectRelationSchema,RelationPageSchema})) z.globalRegistry.add(schema, { id: id.replace(/Schema$/, '') });
