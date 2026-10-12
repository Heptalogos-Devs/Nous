// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { z } from "zod";
import type { ModelRole } from "../roles.js";
import { queryConceptSchema } from "./query-concept.js";
import { formationSchema } from "./formation.js";
import { projectionStewardSchema } from "./projection.js";
import { materialInterpretationSchema } from "./material-interpretation.js";
import {
  episodePartitionSchema,
  journalSynthesisSchema,
} from "./longitudinal.js";
import { conceptMaintenanceSchema } from "./concept-maintenance.js";
import { consolidationSchema } from "./consolidation.js";
import {
  type StructuredOutputContract,
  structuredOutputContract,
} from "./provider.js";
import { materialInterpretationProviderContract } from "./material-interpretation.js";

function contract<S extends z.ZodType>(
  id: string,
  providerName: string,
  owner: S,
  output: StructuredOutputContract<z.output<S>> = structuredOutputContract(
    owner,
  ),
) {
  return { id, providerName, owner, ...output };
}
const material = contract(
  "material.interpretation",
  "material_interpretation",
  materialInterpretationSchema,
  materialInterpretationProviderContract,
);
const roleContracts = {
  query_concept_enrichment: contract(
    "query.concept-enrichment",
    "query_concept_enrichment",
    queryConceptSchema,
  ),
  projection_steward: contract(
    "projection.steward",
    "projection_steward",
    projectionStewardSchema,
  ),
  memory_formation: contract(
    "memory.formation",
    "memory_formation",
    formationSchema,
  ),
  material_structuring: material,
  material_direct_structuring: material,
  episode_segmentation: contract(
    "episode.partition",
    "episode_partition",
    episodePartitionSchema,
  ),
  journal_synthesis: contract(
    "journal.synthesis",
    "journal_synthesis",
    journalSynthesisSchema,
  ),
  concept_maintenance: contract(
    "memory.concept-maintenance",
    "concept_maintenance",
    conceptMaintenanceSchema,
  ),
  memory_consolidation: contract(
    "memory.consolidation",
    "memory_consolidation",
    consolidationSchema,
  ),
  material_description: undefined,
  query_embedding: undefined,
  query_rerank: undefined,
  speech_transcription: undefined,
} as const;
export type ProviderContractFor<R extends ModelRole> =
  (typeof roleContracts)[R];
type OutputForContract<C> = C extends { owner: infer S extends z.ZodType }
  ? z.output<S>
  : string;
export type ModelGenerationOutput<R extends ModelRole> = OutputForContract<
  ProviderContractFor<R>
>;

export function providerContractForRole<R extends ModelRole>(
  role: R,
): ProviderContractFor<R> {
  return roleContracts[role];
}
