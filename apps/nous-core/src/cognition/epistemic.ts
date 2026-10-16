// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import {
  EpistemicEntrySchema,
  EpistemicViewSchema,
  type ContextSegment,
  type Degradation,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

/** Composes only already authorized, selected owner contributions. No second
 * retrieval, inferred competence score, or Host-side knowledge database. */
export function epistemicView(
  segments: readonly ContextSegment[],
  coverage: readonly Degradation[],
) {
  const view = create(EpistemicViewSchema, {
    complete: false,
    coverage: [...coverage],
  });
  for (const source of segments) {
    const entry = create(EpistemicEntrySchema, {
      sourceRefs: source.sourceRefs,
      text: source.text,
      evidence: source.evidence,
      temporal: source.temporal,
      domainKnowledge: source.domainKnowledge,
      epistemicClass: source.epistemicClass,
      attribution:
        "Subject understanding; preserve source reports, uncertainty and exact revision attribution",
    });
    if (source.domainKnowledge) {
      view.familiarDomains.push(entry);
      if (
        source.domainKnowledge.limitations.length ||
        ["unfamiliar", "aware"].includes(source.domainKnowledge.familiarity)
      )
        view.unknowns.push(entry);
      if (source.domainKnowledge.interestRefs.length)
        view.learningOpportunities.push(entry);
    }
    if (
      source.sourceRefs.some((r) =>
        [
          "memory",
          "memory_revision",
          "cognitive_schema",
          "cognitive_schema_revision",
          "language_convention",
          "language_convention_revision",
        ].includes(r.kind),
      )
    )
      view.acceptedFactsAndReports.push(entry);
    if (
      source.sourceRefs.some((r) =>
        ["episode", "episode_revision", "journal", "journal_revision"].includes(
          r.kind,
        ),
      )
    )
      view.experiences.push(entry);
    if (
      source.authority === "evidence" ||
      source.authority === "interpretation"
    ) {
      view.executionAssistance.push(
        create(EpistemicEntrySchema, {
          ...entry,
          attribution:
            "Current supplied information or assistance; contact alone does not establish prior personal experience or mastered ability",
        }),
      );
      if (source.temporal?.temporalEvidence?.sources.length)
        view.experiences.push(entry);
    }
  }
  return view;
}
