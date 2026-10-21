import { z } from 'zod';
import { RefSchema,InstantSchema } from './common.js';
import { KnowledgeKindSchema } from './knowledge.js';
export const SubjectCreateSchema=z.object({id:z.uuidv7().optional(),displayName:z.string().min(1),capabilities:z.array(KnowledgeKindSchema).default(['Memory'])});
export const SubjectSchema=z.object({id:z.uuidv7(),ref:RefSchema,displayName:z.string(),capabilities:z.array(KnowledgeKindSchema),status:z.enum(['active','paused','stopped']),createdAt:InstantSchema,changeCursor:z.string(),branchedFrom:RefSchema.nullable()});
z.globalRegistry.add(SubjectCreateSchema,{id:'SubjectCreate'});
z.globalRegistry.add(SubjectSchema,{id:'Subject'});
