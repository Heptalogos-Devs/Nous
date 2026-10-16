// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

export const roleNames = [
  "projection_steward",
  "memory_formation",
  "self_formation",
  "self_reflection",
  "subject_reflection",
  "episode_segmentation",
  "journal_synthesis",
  "memory_consolidation",
  "concept_maintenance",
  "material_description",
  "material_structuring",
  "material_direct_structuring",
  "query_embedding",
  "query_concept_enrichment",
  "query_rerank",
  "speech_transcription",
] as const;
export type ModelRole = (typeof roleNames)[number];
