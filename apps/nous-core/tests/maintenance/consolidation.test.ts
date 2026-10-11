// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it, vi } from "vitest";
import { create } from "@bufbuild/protobuf";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../../src/kernel-client.js";
import { consolidationSchema } from "../../src/model/schemas/consolidation.js";
import {
  executeConsolidation,
  type ConsolidationActionResult,
} from "../../src/maintenance/consolidation.js";
const occurrence = "10000000-0000-4000-8000-000000000001";
const revision = "20000000-0000-4000-8000-000000000001";
const operation = "40000000-0000-4000-8000-000000000001";
const plan = create(MaintenancePlanSchema, {
  subjectId: "30000000-0000-4000-8000-000000000001",
  authoritySeq: 12n,
  maxConsolidationActions: 8,
  consolidationSource: {
    reference: { kind: "episode_revision", value: revision },
    expectedEpoch: 2n,
  },
  members: [{ key: "member", occurrenceId: occurrence }],
  basis: [
    {
      key: "source",
      basis: {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: occurrence,
            locator: { case: "wholeOccurrence", value: true },
            basisRole: "direct",
          },
        },
      },
    },
  ],
  candidates: [
    {
      key: "target",
      cognitiveRole: "declarative",
      formationMode: "grounded",
      eligibleBasisKeys: ["source"],
      target: {
        reference: { kind: "memory_revision", value: revision },
        objectId: "memory-owner-id",
        expectedEpoch: 7n,
      },
    },
  ],
});
const content = {
  cognitiveRole: "declarative",
  formationMode: "grounded",
  groundingMemberKey: "member",
  semanticRole: "statement",
  text: "A useful supported fact.",
  title: null,
  basisKeys: ["source"],
  entityKeys: [],
  validTime: { kind: "unknown" },
  epistemicClass: "derived",
};
const schemaContent = {
  title: null,
  structuralClaim: "A bounded pattern",
  applicability: "These observations",
  boundaryDefinition: "Only this scope",
  formationKind: "explicit_import",
  entityKeys: [],
  validTime: { kind: "unknown" },
  evidence: [{ role: "support", basisKey: "source" }],
};
function fixture() {
  const form = vi.fn(
    async (_request: Parameters<KernelClient["memory"]["formMemory"]>[0]) => ({
      header: { currentRevisionRef: { value: "formed-revision" } },
    }),
  );
  const revise = vi.fn(async () => ({
    header: { currentRevisionRef: { value: "revised-revision" } },
  }));
  const link = vi.fn(async () => ({}));
  const schema = vi.fn(async () => ({
    header: { currentRevisionRef: { value: "schema-revision" } },
  }));
  const kernel = {
    memory: { formMemory: form, reviseMemory: revise, linkRevisions: link },
    concepts: { createCognitiveSchema: schema },
  } as unknown as KernelClient;
  let progress: ConsolidationActionResult[] = [];
  const save = vi.fn(async (results: ConsolidationActionResult[]) => {
    progress = [...results];
  });
  return { kernel, form, revise, link, schema, save, progress: () => progress };
}
it("uses canonical owner APIs and returned earlier-action refs for strong Memory relations", async () => {
  const f = fixture();
  const proposal = consolidationSchema.parse({
    actions: [
      { action: "create_memory", content },
      {
        action: "revise_memory",
        targetKey: "target",
        intent: "correct",
        content,
      },
      {
        action: "link_relation",
        from: { kind: "action", index: 0 },
        to: { kind: "action", index: 1 },
        relation: "elaborates",
      },
      { action: "create_schema", content: schemaContent },
    ],
  });
  const outcome = await executeConsolidation(
    f.kernel,
    plan,
    proposal,
    operation,
    create(ProducerSignatureSchema),
    {},
    [],
    f.save,

    (_id, commit) => commit(),
  );
  expect(outcome.status).toBe("committed");
  expect(f.form.mock.calls[0]?.[0].input?.groundingOccurrenceId).toBe(
    occurrence,
  );
  expect(f.revise).toHaveBeenCalledWith(
    expect.objectContaining({
      memoryId: "memory-owner-id",
      expectedObjectEpoch: 7n,
    }),
    {},
  );
  expect(f.link).toHaveBeenCalledWith(
    expect.objectContaining({
      fromRevisionId: "formed-revision",
      toRevisionId: "revised-revision",
    }),
    {},
  );
  expect(new Set(f.progress().map((r) => r.index)).size).toBe(4);
});
